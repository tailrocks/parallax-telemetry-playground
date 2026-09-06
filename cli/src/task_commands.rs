//! Rust-owned replacements for repository shell entrypoints.
//!
//! Mise is the public task surface. This module owns the process, HTTP, and
//! validation plumbing behind those tasks so scenario behavior does not depend
//! on a shell, shell traps, or shell argument parsing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Router, serve};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::{commerce_verify, scenario_runner, test_verify};

fn root() -> PathBuf {
    let compiled_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli is a workspace member");
    if compiled_root.is_dir() {
        compiled_root.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
    }
}

async fn output(program: &str, args: &[String], cwd: &Path) -> anyhow::Result<Output> {
    output_with_env(program, args, cwd, &[]).await
}

async fn output_with_env(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(&str, &str)],
) -> anyhow::Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().copied())
        .output()
        .await
        .with_context(|| format!("failed to start {program}"))
}

async fn status(program: &str, args: &[String], cwd: &Path) -> anyhow::Result<()> {
    status_with_env(program, args, cwd, &[]).await
}

async fn status_with_env(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(&str, &str)],
) -> anyhow::Result<()> {
    status_with_env_clearing(program, args, cwd, env, &[]).await
}

async fn status_with_env_clearing(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(&str, &str)],
    clear_env: &[&str],
) -> anyhow::Result<()> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().copied())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for name in clear_env {
        command.env_remove(name);
    }
    let status = command
        .status()
        .await
        .with_context(|| format!("failed to start {program}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{program} exited with {}", status.code().unwrap_or(1));
    }
}

async fn status_with_input(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(&str, &str)],
    input: &str,
) -> anyhow::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    let mut stdin = child
        .stdin
        .take()
        .context("started child without a writable stdin")?;
    stdin.write_all(input.as_bytes()).await?;
    drop(stdin);
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        bail!("{program} exited with {}", status.code().unwrap_or(1));
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn env_or_fallback(primary: &str, fallback: &str, default: &str) -> String {
    nonempty_env(primary)
        .or_else(|| nonempty_env(fallback))
        .unwrap_or_else(|| default.to_owned())
}

fn positive_env(name: &str, default: u64) -> anyhow::Result<u64> {
    let raw = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let value = raw
        .parse::<u64>()
        .with_context(|| format!("{name} must be a positive integer: {raw}"))?;
    ensure!(value > 0, "{name} must be a positive integer: {raw}");
    Ok(value)
}

fn positive_env_fallback(primary: &str, fallback: &str, default: u64) -> anyhow::Result<u64> {
    let raw = nonempty_env(primary)
        .or_else(|| nonempty_env(fallback))
        .unwrap_or_else(|| default.to_string());
    let value = raw
        .parse::<u64>()
        .with_context(|| format!("{primary} must be a positive integer: {raw}"))?;
    ensure!(value > 0, "{primary} must be a positive integer: {raw}");
    Ok(value)
}

struct PostgresEnv {
    host: String,
    port: String,
    database: String,
    user: String,
    password: String,
    connect_timeout: String,
    options: String,
}

impl PostgresEnv {
    fn new(statement_timeout_ms: u64, lock_timeout_ms: u64, connect_timeout: u64) -> Self {
        let bounds = format!(
            "-c statement_timeout={statement_timeout_ms}ms -c lock_timeout={lock_timeout_ms}ms"
        );
        let options = match nonempty_env("PGOPTIONS") {
            Some(existing) => format!("{existing} {bounds}"),
            None => bounds,
        };
        Self {
            host: env_or_fallback("PGHOST", "POSTGRES_HOST", "postgres"),
            port: env_or_fallback("PGPORT", "POSTGRES_PORT", "5432"),
            database: env_or_fallback("PGDATABASE", "POSTGRES_DB", "playground"),
            user: env_or_fallback("PGUSER", "POSTGRES_USER", "postgres"),
            password: env_or_fallback("PGPASSWORD", "POSTGRES_PASSWORD", ""),
            connect_timeout: connect_timeout.to_string(),
            options,
        }
    }

    fn pairs(&self) -> [(&str, &str); 7] {
        [
            ("PGHOST", &self.host),
            ("PGPORT", &self.port),
            ("PGDATABASE", &self.database),
            ("PGUSER", &self.user),
            ("PGPASSWORD", &self.password),
            ("PGCONNECT_TIMEOUT", &self.connect_timeout),
            ("PGOPTIONS", &self.options),
        ]
    }
}

async fn wait_for_postgres(
    repository: &Path,
    postgres: &PostgresEnv,
    timeout_seconds: u64,
    poll_seconds: u64,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let pg_isready = postgres_program("pg_isready");
    loop {
        let ready =
            output_with_env(&pg_isready, &["-q".into()], repository, &postgres.pairs()).await?;
        if ready.status.success() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "PostgreSQL did not become ready within {timeout_seconds}s (host={} port={} database={} user={})",
                postgres.host,
                postgres.port,
                postgres.database,
                postgres.user
            );
        }
        tokio::time::sleep(Duration::from_secs(poll_seconds)).await;
    }
}

fn migration_version(path: &Path) -> anyhow::Result<String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("migration filename is not valid UTF-8")?;
    let version = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("migration filename is not valid UTF-8")?;
    ensure!(
        path.is_file()
            && path.extension().is_some_and(|extension| extension == "sql")
            && !version.is_empty()
            && version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
        "invalid migration filename: {name}"
    );
    Ok(version.to_owned())
}

fn sql_values(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .map(|value| format!("('{}')", value.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn postgres_program(name: &str) -> String {
    // The official Postgres image keeps client binaries in a versioned
    // directory. Resolve that location explicitly when the image advertises
    // its major version, then retain PATH lookup for host-side use.
    if let Ok(major) = std::env::var("PG_MAJOR") {
        let versioned_path = Path::new("/usr/lib/postgresql")
            .join(major)
            .join("bin")
            .join(name);
        if versioned_path.is_file() {
            return versioned_path.display().to_string();
        }
    }
    name.to_owned()
}

const REQUIRED_POSTGRES_TABLES: &[&str] = &[
    "tenants",
    "categories",
    "products",
    "product_variants",
    "prices",
    "promotions",
    "promotion_products",
    "customers",
    "reviews",
    "inventory_locations",
    "inventory",
    "carts",
    "cart_items",
    "orders",
    "checkout_attempts",
    "order_items",
    "payments",
    "payment_provider_authorization_decisions",
    "shipments",
    "payment_operation_requests",
    "shipment_items",
    "outbox_events",
    "feature_exposures",
    "analytics_events",
    "notification_deliveries",
    "fulfillment_processed_events",
    "price_change_events",
    "inventory_reservations",
    "checkout_compensation_tasks",
    "checkout_payment_reconciliations",
    "fulfillment_effects",
    "notification_delivery_attempts",
    "notification_channel_messages",
    "orders_consumer_inbox",
    "schema_migrations",
];

const REQUIRED_POSTGRES_COLUMNS: &[(&str, &str)] = &[
    ("tenants", "id"),
    ("tenants", "default_currency"),
    ("products", "tenant_id"),
    ("product_variants", "sku"),
    ("prices", "amount_minor"),
    ("prices", "valid_from"),
    ("customers", "tenant_id"),
    ("inventory", "available_quantity"),
    ("carts", "customer_id"),
    ("cart_items", "unit_price_minor"),
    ("orders", "tenant_id"),
    ("orders", "total_minor"),
    ("orders", "checkout_request_id"),
    ("checkout_attempts", "request_fingerprint"),
    ("checkout_attempts", "lease_token"),
    ("checkout_attempts", "response_payload"),
    ("order_items", "line_total_minor"),
    ("payments", "status"),
    ("payments", "captured_amount_minor"),
    ("payments", "pending_resolution"),
    ("payments", "pending_reconciliation_attempts"),
    ("payments", "pending_reconciliation_at"),
    ("payments", "pending_resolution_at"),
    ("payments", "provider_decision_verified_at"),
    ("payment_operation_requests", "request_fingerprint"),
    ("shipments", "status"),
    ("shipment_items", "quantity"),
    ("outbox_events", "available_at"),
    ("outbox_events", "traceparent"),
    ("outbox_events", "tracestate"),
    ("outbox_events", "baggage"),
    ("outbox_events", "occurred_at"),
    ("outbox_events", "failure_code"),
    ("outbox_events", "failure_message"),
    ("outbox_events", "failed_at"),
    ("outbox_events", "claim_token"),
    ("outbox_events", "claim_expires_at"),
    ("feature_exposures", "variant"),
    ("analytics_events", "trace_id"),
    ("notification_deliveries", "event_key"),
    ("notification_deliveries", "next_attempt_at"),
    ("notification_deliveries", "lease_until"),
    ("notification_deliveries", "lease_token"),
    ("notification_deliveries", "last_error"),
    ("notification_deliveries", "dead_lettered_at"),
    ("notification_deliveries", "acknowledged_at"),
    ("notification_deliveries", "updated_at"),
    ("notification_deliveries", "traceparent"),
    ("notification_deliveries", "tracestate"),
    ("notification_deliveries", "baggage"),
    ("fulfillment_processed_events", "claimed_at"),
    ("fulfillment_processed_events", "lease_token"),
    ("fulfillment_processed_events", "lease_until"),
    ("fulfillment_processed_events", "dead_lettered_at"),
    ("price_change_events", "sequence"),
    ("inventory_reservations", "reservation_id"),
    ("inventory_reservations", "status"),
    ("inventory_reservations", "expires_at"),
    ("inventory_reservations", "consumed_at"),
    ("inventory_reservations", "owner_request_id"),
    ("inventory_reservations", "owner_lease_token"),
    ("checkout_compensation_tasks", "task_key"),
    ("checkout_compensation_tasks", "status"),
    ("checkout_compensation_tasks", "checkout_request_id"),
    ("checkout_compensation_tasks", "checkout_lease_token"),
    ("checkout_compensation_tasks", "remote_operation_id"),
    ("checkout_compensation_tasks", "remote_operation_started_at"),
    (
        "checkout_compensation_tasks",
        "remote_operation_completed_at",
    ),
    ("checkout_compensation_tasks", "traceparent"),
    ("checkout_compensation_tasks", "tracestate"),
    ("checkout_compensation_tasks", "baggage"),
    ("checkout_payment_reconciliations", "tenant_id"),
    ("checkout_payment_reconciliations", "request_id"),
    ("checkout_payment_reconciliations", "order_id"),
    ("checkout_payment_reconciliations", "authorize_request_id"),
    ("checkout_payment_reconciliations", "payment_id"),
    ("checkout_payment_reconciliations", "merchant_reference"),
    ("checkout_payment_reconciliations", "amount_minor"),
    ("checkout_payment_reconciliations", "currency"),
    ("checkout_payment_reconciliations", "method_type"),
    ("checkout_payment_reconciliations", "feature_variant"),
    ("checkout_payment_reconciliations", "status"),
    ("checkout_payment_reconciliations", "attempts"),
    ("checkout_payment_reconciliations", "available_at"),
    ("checkout_payment_reconciliations", "claimed_at"),
    ("checkout_payment_reconciliations", "lease_token"),
    ("checkout_payment_reconciliations", "lease_expires_at"),
    ("checkout_payment_reconciliations", "checkout_lease_token"),
    ("checkout_payment_reconciliations", "completed_at"),
    ("checkout_payment_reconciliations", "last_payment_status"),
    ("checkout_payment_reconciliations", "last_operation_status"),
    ("checkout_payment_reconciliations", "last_failure_reason"),
    ("checkout_payment_reconciliations", "last_error"),
    ("checkout_payment_reconciliations", "traceparent"),
    ("checkout_payment_reconciliations", "tracestate"),
    ("checkout_payment_reconciliations", "baggage"),
    ("checkout_payment_reconciliations", "created_at"),
    ("checkout_payment_reconciliations", "updated_at"),
    ("fulfillment_effects", "tenant_id"),
    ("fulfillment_effects", "order_id"),
    ("fulfillment_effects", "effect_key"),
    ("fulfillment_effects", "operation"),
    ("fulfillment_effects", "effect_kind"),
    ("fulfillment_effects", "payload"),
    ("fulfillment_effects", "notification_status"),
    ("fulfillment_effects", "aggregate_version"),
    ("fulfillment_effects", "status"),
    ("fulfillment_effects", "attempts"),
    ("fulfillment_effects", "claim_token"),
    ("fulfillment_effects", "claim_until"),
    ("fulfillment_effects", "published_at"),
    ("fulfillment_effects", "cancelled_at"),
    ("fulfillment_effects", "last_error"),
    ("fulfillment_effects", "created_at"),
    ("notification_delivery_attempts", "id"),
    ("notification_delivery_attempts", "delivery_id"),
    ("notification_delivery_attempts", "tenant_id"),
    ("notification_delivery_attempts", "attempt"),
    ("notification_delivery_attempts", "outcome"),
    ("notification_delivery_attempts", "error"),
    ("notification_delivery_attempts", "started_at"),
    ("notification_delivery_attempts", "completed_at"),
    ("notification_channel_messages", "delivery_id"),
    ("notification_channel_messages", "tenant_id"),
    ("notification_channel_messages", "order_id"),
    ("notification_channel_messages", "channel"),
    ("notification_channel_messages", "payload"),
    ("notification_channel_messages", "dispatched_at"),
    ("notification_channel_messages", "acknowledged_at"),
    ("notification_channel_messages", "acknowledgement_reference"),
    ("payment_provider_authorization_decisions", "tenant_id"),
    ("payment_provider_authorization_decisions", "payment_id"),
    ("payment_provider_authorization_decisions", "provider"),
    (
        "payment_provider_authorization_decisions",
        "provider_reference",
    ),
    ("payment_provider_authorization_decisions", "amount_minor"),
    ("payment_provider_authorization_decisions", "currency"),
    ("payment_provider_authorization_decisions", "outcome"),
    ("payment_provider_authorization_decisions", "verified_at"),
    ("payment_provider_authorization_decisions", "created_at"),
    ("orders_consumer_inbox", "tenant_id"),
    ("orders_consumer_inbox", "event_id"),
    ("orders_consumer_inbox", "status"),
    ("orders_consumer_inbox", "attempts"),
    ("orders_consumer_inbox", "lease_until"),
];

fn is_snake_case(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        && !value.contains("__")
}

fn is_semantic_name(name: &str) -> bool {
    let mut parts = name.split(':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(group), Some(scenario), None) => is_snake_case(group) && is_snake_case(scenario),
        _ => false,
    }
}

fn is_public_scenario_name(name: &str) -> bool {
    name == "corpus:all" || scenario_runner::semantic_names().contains(&name)
}

fn validate_scenario_task(task: &Value, name: &str) -> anyhow::Result<()> {
    if !is_semantic_name(name) {
        bail!("scenario task name must be exactly group:snake_case: {name}");
    }

    let description = task
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    if description.trim().is_empty() {
        bail!("semantic scenario task has no description: {name}");
    }

    let aliases = task
        .get("aliases")
        .and_then(Value::as_array)
        .context("mise task has no aliases array")?;
    if !aliases.is_empty() {
        bail!("opaque task alias remains for {name}");
    }

    if task.get("shell").is_some_and(|shell| !shell.is_null()) {
        bail!("scenario task must not configure a shell: {name}");
    }
    if task.get("raw").and_then(Value::as_bool) != Some(true) {
        bail!("scenario task must forward raw arguments: {name}");
    }

    let runs = task
        .get("run")
        .and_then(Value::as_array)
        .context("mise scenario task has no run array")?;
    if runs.len() != 1 {
        bail!("scenario task must have exactly one Rust run command: {name}");
    }
    let run = runs
        .first()
        .and_then(Value::as_str)
        .context("mise scenario task run entry is not a command string")?;
    let expected = format!("cargo run --locked -p playground-cli -- scenario {name}");
    if run != expected {
        bail!("scenario task {name} must run `{expected}` without a shell; got `{run}`");
    }
    Ok(())
}

pub(crate) async fn check_scenarios() -> anyhow::Result<i32> {
    let repository = root();
    scenario_runner::validate_scenario_registry()?;
    let scenarios = repository.join("scenarios");
    let mut legacy_files_args = vec![
        "ls-files".into(),
        "--cached".into(),
        "--others".into(),
        "--exclude-standard".into(),
        "--".into(),
    ];
    legacy_files_args.extend(
        [
            "*.sh",
            "*.bash",
            "*.zsh",
            "*.bat",
            "*.cmd",
            "gradlew",
            "**/gradlew",
        ]
        .into_iter()
        .map(String::from),
    );
    let shell_files_output = output("git", &legacy_files_args, &repository).await?;
    ensure!(
        shell_files_output.status.success(),
        "git ls-files legacy entrypoint scan failed"
    );
    let shell_files = String::from_utf8_lossy(&shell_files_output.stdout)
        .lines()
        .map(|path| repository.join(path))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if let Some(path) = shell_files.first() {
        bail!(
            "repository legacy script or wrapper remains: {}; migrate it to Rust/mise",
            path.display()
        );
    }
    if scenarios.join("run.sh").exists() {
        bail!("legacy scenario dispatcher remains: scenarios/run.sh");
    }

    let tasks = output(
        "mise",
        &[
            "tasks".into(),
            "ls".into(),
            "--local".into(),
            "--json".into(),
        ],
        &repository,
    )
    .await?;
    if !tasks.status.success() {
        bail!(
            "mise tasks ls failed: {}",
            String::from_utf8_lossy(&tasks.stderr).trim()
        );
    }
    let tasks: Vec<Value> =
        serde_json::from_slice(&tasks.stdout).context("mise returned invalid task JSON")?;
    let semantic_names = scenario_runner::semantic_names();
    for task in &tasks {
        let Some(name) = task.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !is_semantic_name(name) {
            bail!("local mise task name must be exactly group:snake_case: {name}");
        }
        if task
            .get("description")
            .and_then(Value::as_str)
            .is_none_or(|description| description.trim().is_empty())
        {
            bail!("local mise task has no description: {name}");
        }
        let aliases = task
            .get("aliases")
            .and_then(Value::as_array)
            .context("mise task has no aliases array")?;
        if !aliases.is_empty() {
            bail!("opaque task alias remains for {name}");
        }
        let invokes_scenario = task
            .get("run")
            .and_then(Value::as_array)
            .is_some_and(|runs| {
                runs.iter().any(|run| {
                    run.as_str()
                        .is_some_and(|command| command.contains(" -- scenario "))
                })
            });
        if invokes_scenario && !is_public_scenario_name(name) {
            bail!("unregistered public scenario task: {name}");
        }
    }

    for name in semantic_names
        .iter()
        .copied()
        .chain(std::iter::once("corpus:all"))
    {
        let matches = tasks
            .iter()
            .filter(|task| task.get("name").and_then(Value::as_str) == Some(name))
            .collect::<Vec<_>>();
        let task = match matches.as_slice() {
            [] => bail!("semantic scenario task is missing: {name}"),
            [task] => *task,
            _ => bail!("semantic scenario task is duplicated: {name}"),
        };
        validate_scenario_task(task, name)?;
    }

    let readme = fs::read_to_string(repository.join("scenarios/README.md"))?;
    for forbidden in [
        "scenarios/run.sh",
        "short scenario IDs remain aliases",
        ".sh`",
    ] {
        if readme.contains(forbidden) {
            bail!("scenario README retains forbidden legacy reference: {forbidden}");
        }
    }
    let runner_source = fs::read_to_string(repository.join("cli/src/scenario_runner.rs"))?;
    let dispatch_source = runner_source
        .split_once("async fn run_named")
        .and_then(|(_, rest)| rest.split_once("async fn current_executable"))
        .map(|(body, _)| body)
        .context("scenario runner dispatch function is missing")?;
    for name in semantic_names {
        ensure!(
            dispatch_source.contains(&format!("\"{name}\"")),
            "semantic scenario has no Rust dispatch arm: {name}"
        );
    }
    println!(
        "scenario contract is complete: {} semantic mise tasks plus corpus:all; Rust dispatch validated",
        semantic_names.len()
    );
    Ok(0)
}

pub(crate) async fn check_typescript_policy() -> anyhow::Result<i32> {
    let repository = root();
    let tracked = output(
        "git",
        &[
            "ls-files".into(),
            "*.js".into(),
            "*.jsx".into(),
            "*.mjs".into(),
            "*.cjs".into(),
            "*.mts".into(),
            "*.cts".into(),
        ],
        &repository,
    )
    .await?;
    if !tracked.status.success() {
        bail!("git ls-files failed");
    }
    let tracked_text = String::from_utf8_lossy(&tracked.stdout).into_owned();
    let forbidden = tracked_text
        .lines()
        .filter(|path| repository.join(path).exists())
        .collect::<Vec<_>>();
    if !forbidden.is_empty() {
        bail!(
            "tracked JavaScript source/config is forbidden: {}",
            forbidden.join(", ")
        );
    }

    let config: Value =
        serde_json::from_str(&fs::read_to_string(repository.join("web/tsconfig.json"))?)?;
    let compiler = config
        .get("compilerOptions")
        .context("tsconfig.compilerOptions is missing")?;
    for option in [
        "strict",
        "noUncheckedIndexedAccess",
        "exactOptionalPropertyTypes",
        "noImplicitOverride",
        "noImplicitReturns",
        "noPropertyAccessFromIndexSignature",
        "noUnusedLocals",
        "noUnusedParameters",
        "noFallthroughCasesInSwitch",
        "noUncheckedSideEffectImports",
        "forceConsistentCasingInFileNames",
        "isolatedModules",
        "noEmit",
    ] {
        if compiler.get(option).and_then(Value::as_bool) != Some(true) {
            bail!("tsconfig compilerOptions.{option} must be true");
        }
    }
    for option in [
        "allowJs",
        "checkJs",
        "allowUnusedLabels",
        "allowUnreachableCode",
    ] {
        if compiler.get(option).and_then(Value::as_bool) != Some(false) {
            bail!("tsconfig compilerOptions.{option} must be false");
        }
    }
    if compiler.get("moduleDetection").and_then(Value::as_str) != Some("force") {
        bail!("tsconfig compilerOptions.moduleDetection must be force");
    }

    let web = repository.join("web");
    status("bun", &["run".into(), "typecheck".into()], &web).await?;
    println!("strict TypeScript-only policy passed");
    Ok(0)
}

pub(crate) async fn check_typescript() -> anyhow::Result<i32> {
    let repository = root();
    status(
        "bun",
        &["run".into(), "typecheck".into()],
        &repository.join("web"),
    )
    .await?;
    println!("TypeScript typecheck passed");
    Ok(0)
}

pub(crate) async fn verify_trace(_args: Vec<String>) -> anyhow::Result<i32> {
    let invocation_id = std::env::var("CLI_INVOCATION_ID")
        .context("CLI_INVOCATION_ID is required; run through parallax invocation start")?;
    let traceparent = std::env::var("TRACEPARENT")
        .context("TRACEPARENT is required; run through parallax invocation start")?;
    let trace_id = validate_traceparent(&traceparent)?;
    let tracestate = std::env::var("TRACESTATE").unwrap_or_else(|_| "playground=commerce".into());
    let mut baggage = std::env::var("BAGGAGE").unwrap_or_default();
    if !baggage.contains("cli.invocation.id=") {
        if !baggage.is_empty() {
            baggage.push(',');
        }
        baggage.push_str("cli.invocation.id=");
        baggage.push_str(&invocation_id);
    }
    let static_baggage = "tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal";
    if !baggage.is_empty() {
        baggage.push(',');
    }
    baggage.push_str(static_baggage);
    if baggage.len() > 2048 {
        bail!("generated baggage exceeds 2048 bytes");
    }

    let checkout_url =
        std::env::var("CHECKOUT_URL").unwrap_or_else(|_| "http://localhost:8088".into());
    let request = serde_json::json!({
        "tenant_id": "tenant-acme",
        "customer_id": "customer-acme-ava",
        "items": [{"sku": "WIDGET-1", "quantity": 1}],
        "currency_code": "USD",
        "payment_method_token": "tok_visa",
        "payment_method_type": "card",
        "request_id": format!("commerce-verification-{invocation_id}"),
    });
    let response = reqwest::Client::new()
        .post(format!("{checkout_url}/checkout"))
        .header("traceparent", &traceparent)
        .header("tracestate", &tracestate)
        .header("baggage", &baggage)
        .json(&request)
        .send()
        .await
        .context("commerce verification checkout request failed")?;
    let http_status = response.status();
    let body: Value = response
        .json()
        .await
        .context("checkout returned invalid JSON")?;
    if !http_status.is_success()
        || body.get("status").and_then(Value::as_str) != Some("paid")
        || body
            .get("order_id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        bail!("commerce checkout verification failed: HTTP {http_status}: {body}");
    }
    let api_url =
        std::env::var("PARALLAX_API_URL").unwrap_or_else(|_| "http://127.0.0.1:4000".into());
    let summary = commerce_verify::verify(&api_url, &trace_id).await?;
    println!(
        "verified commerce trace {trace_id}: {} linked trace(s), {} spans, services={}",
        summary.traces,
        summary.spans,
        summary.services.join(",")
    );
    Ok(0)
}

fn validate_traceparent(value: &str) -> anyhow::Result<String> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0] != "00"
        || parts[1].len() != 32
        || parts[2].len() != 16
        || parts[3].len() != 2
        || !parts[1].bytes().all(|byte| byte.is_ascii_hexdigit())
        || !parts[2].bytes().all(|byte| byte.is_ascii_hexdigit())
        || parts[1].bytes().all(|byte| byte == b'0')
        || parts[2].bytes().all(|byte| byte == b'0')
    {
        bail!("invalid traceparent: expected complete version-00 W3C value");
    }
    Ok(parts[1].to_owned())
}

const REQUIRED_RUNNING_COMPOSE_SERVICES: &[&str] = &[
    "postgres",
    "redis",
    "rabbitmq",
    "clickhouse",
    "flagd",
    "catalog",
    "payment",
    "fulfillment",
    "checkout",
    "pricing",
    "inventory",
    "recommendation",
    "notifications",
    "orders",
    "storefront",
    "web",
];

const REQUIRED_COMPLETED_COMPOSE_SERVICES: &[&str] =
    &["flagd-health-tools", "postgres-migrate", "clickhouse-init"];

const POSTGRES_PROBE: &[&str] = &["pg_isready", "-U", "postgres", "-d", "playground"];
const REDIS_PROBE: &[&str] = &["redis-cli", "-h", "127.0.0.1", "-p", "6379", "ping"];
const RABBITMQ_PROBE: &[&str] = &["su-exec", "rabbitmq", "rabbitmq-diagnostics", "-q", "ping"];
const CLICKHOUSE_PROBE: &[&str] = &[
    "sh",
    "-ec",
    "clickhouse-client --host localhost --user \"${CLICKHOUSE_USER:-default}\" --password \"${CLICKHOUSE_PASSWORD:-}\" --query 'SELECT 1' >/dev/null",
];
const FLAGD_PROBE: &[&str] = &[
    "/health-tools/busybox",
    "wget",
    "-q",
    "-O",
    "/dev/null",
    "http://127.0.0.1:8014/healthz",
];
const PRICING_GRPC_PROBE: &[&str] = &[
    "/usr/local/bin/grpc_health_probe",
    "-addr=127.0.0.1:50051",
    "-service=playground.pricing.v1.Pricing",
    "-connect-timeout=2s",
    "-rpc-timeout=2s",
];
const PAYMENT_HTTP_PROBE: &[&str] = &[
    "curl",
    "--fail",
    "--silent",
    "--show-error",
    "http://127.0.0.1:8080/actuator/health",
];
const PAYMENT_GRPC_PROBE: &[&str] = &[
    "/usr/local/bin/grpc_health_probe",
    "-addr=127.0.0.1:9090",
    "-service=playground.payment.v1.Payment",
    "-connect-timeout=2s",
    "-rpc-timeout=2s",
];
const NOTIFICATIONS_HTTP_PROBE: &[&str] = &[
    "sh",
    "-ec",
    "printf 'GET /healthz HTTP/1.0\\r\\nHost: localhost\\r\\n\\r\\n' | nc -w 2 127.0.0.1 8091 | grep -q ' 200 '",
];

const WEB_ACCEPTANCE_COMMAND: &[&str] = &["run", "e2e:compose"];
const WEB_ACCEPTANCE_ENV: &[(&str, &str)] = &[("PLAYGROUND_COMPOSE_E2E", "1")];
const WEB_ACCEPTANCE_CLEAR_ENV: &[&str] = &["PLAYGROUND_MOCK_E2E"];
const WEB_MOCK_COMMAND: &[&str] = &[
    "./node_modules/@playwright/test/cli.js",
    "test",
    "--project=chromium",
];
const WEB_MOCK_ENV: &[(&str, &str)] = &[("PLAYGROUND_MOCK_E2E", "1")];

#[derive(Debug, PartialEq, Eq)]
struct ComposeServiceStatus {
    service: String,
    state: String,
    health: Option<String>,
    exit_code: Option<i64>,
}

struct WebObservablePlan {
    command: &'static [&'static str],
    env: &'static [(&'static str, &'static str)],
    clear_env: &'static [&'static str],
}

fn web_observable_plan(acceptance: bool) -> WebObservablePlan {
    if acceptance {
        WebObservablePlan {
            command: WEB_ACCEPTANCE_COMMAND,
            env: WEB_ACCEPTANCE_ENV,
            clear_env: WEB_ACCEPTANCE_CLEAR_ENV,
        }
    } else {
        WebObservablePlan {
            command: WEB_MOCK_COMMAND,
            env: WEB_MOCK_ENV,
            clear_env: &[],
        }
    }
}

fn stack_http_defaults() -> [(&'static str, &'static str, &'static str); 10] {
    [
        (
            "Parallax",
            "PARALLAX_API_URL",
            "http://127.0.0.1:4000/health",
        ),
        ("Checkout", "CHECKOUT_URL", "http://127.0.0.1:8088/readyz"),
        (
            "Catalog",
            "CATALOG_URL",
            "http://127.0.0.1:8080/actuator/health/readiness",
        ),
        (
            "Inventory",
            "INVENTORY_URL",
            "http://127.0.0.1:8089/healthz",
        ),
        (
            "Recommendation",
            "RECOMMENDATION_URL",
            "http://127.0.0.1:8090/readyz",
        ),
        ("Orders", "ORDERS_URL", "http://127.0.0.1:8092/healthz"),
        (
            "Fulfillment",
            "FULFILLMENT_URL",
            "http://127.0.0.1:8093/actuator/health",
        ),
        (
            "Storefront",
            "STOREFRONT_URL",
            "http://127.0.0.1:8094/readyz",
        ),
        (
            "Storefront analytics",
            "STOREFRONT_ANALYTICS_URL",
            "http://127.0.0.1:8094/analytics/readyz",
        ),
        ("Web", "WEB_URL", "http://127.0.0.1:5173/healthz"),
    ]
}

fn parse_compose_statuses(output: &str) -> anyhow::Result<Vec<ComposeServiceStatus>> {
    let trimmed = output.trim();
    ensure!(
        !trimmed.is_empty(),
        "Compose stack returned no container status"
    );
    let values = if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<Value>>(trimmed).context("invalid Compose status array")?
    } else {
        trimmed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Value>(line).context("invalid Compose status row"))
            .collect::<anyhow::Result<Vec<_>>>()?
    };
    values
        .into_iter()
        .map(|value| {
            Ok(ComposeServiceStatus {
                service: value
                    .get("Service")
                    .and_then(Value::as_str)
                    .context("Compose status row has no Service")?
                    .to_owned(),
                state: value
                    .get("State")
                    .and_then(Value::as_str)
                    .context("Compose status row has no State")?
                    .to_owned(),
                health: value
                    .get("Health")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned),
                exit_code: value.get("ExitCode").and_then(Value::as_i64),
            })
        })
        .collect()
}

fn verify_compose_state(output: &str) -> anyhow::Result<()> {
    let statuses = parse_compose_statuses(output)?;
    ensure!(!statuses.is_empty(), "Compose stack has no containers");
    for service in REQUIRED_RUNNING_COMPOSE_SERVICES {
        let status = statuses
            .iter()
            .find(|status| status.service == *service)
            .with_context(|| format!("Compose service is missing: {service}"))?;
        ensure!(
            status.state == "running",
            "Compose service {service} is not running: state={}",
            status.state
        );
        ensure!(
            status.health.as_deref() == Some("healthy"),
            "Compose service {service} is not healthy: health={:?}",
            status.health
        );
    }
    for service in REQUIRED_COMPLETED_COMPOSE_SERVICES {
        let status = statuses
            .iter()
            .find(|status| status.service == *service)
            .with_context(|| format!("Compose job is missing: {service}"))?;
        ensure!(
            status.state == "exited" && status.exit_code == Some(0),
            "Compose job {service} did not complete successfully: state={}, exit_code={:?}",
            status.state,
            status.exit_code
        );
    }
    Ok(())
}

fn compose_command(compose: &Path, args: &[&str]) -> Vec<String> {
    let mut command = vec![
        "compose".to_owned(),
        "-f".to_owned(),
        compose.display().to_string(),
    ];
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    command
}

async fn compose_exec(
    compose: &Path,
    repository: &Path,
    service: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let mut args = compose_command(compose, &["exec", "-T", service]);
    args.extend(command.iter().map(|arg| (*arg).to_owned()));
    status("docker", &args, repository)
        .await
        .with_context(|| format!("Compose probe failed for {service}"))
}

async fn verify_compose_dependency_surfaces(
    compose: &Path,
    repository: &Path,
) -> anyhow::Result<()> {
    for (name, service, command) in [
        ("PostgreSQL", "postgres", POSTGRES_PROBE),
        ("Redis", "redis", REDIS_PROBE),
        ("RabbitMQ", "rabbitmq", RABBITMQ_PROBE),
        ("ClickHouse", "clickhouse", CLICKHOUSE_PROBE),
        ("flagd", "flagd", FLAGD_PROBE),
        ("Pricing gRPC", "pricing", PRICING_GRPC_PROBE),
        ("Payment HTTP", "payment", PAYMENT_HTTP_PROBE),
        ("Payment gRPC", "payment", PAYMENT_GRPC_PROBE),
        (
            "Notifications HTTP",
            "notifications",
            NOTIFICATIONS_HTTP_PROBE,
        ),
    ] {
        compose_exec(compose, repository, service, command)
            .await
            .with_context(|| format!("{name} dependency surface is unavailable"))?;
    }
    Ok(())
}

async fn check_http_endpoint(
    client: &reqwest::Client,
    name: &str,
    url: &str,
) -> anyhow::Result<()> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("{name} health request failed at {url}"))?;
    ensure!(
        response.status().is_success(),
        "{name} health failed at {url}: HTTP {}",
        response.status()
    );
    Ok(())
}

pub(crate) async fn verify_stack(_args: Vec<String>) -> anyhow::Result<i32> {
    let repository = root();
    let compose = repository.join("deploy/docker-compose.yml");
    if !compose.exists() {
        bail!("Compose file is missing: {}", compose.display());
    }
    status(
        "docker",
        &[
            "compose".into(),
            "-f".into(),
            compose.display().to_string(),
            "config".into(),
            "--quiet".into(),
        ],
        &repository,
    )
    .await?;
    if std::env::var("VERIFY_MANAGE_STACK").unwrap_or_else(|_| "0".into()) == "1" {
        status(
            "docker",
            &compose_command(
                compose.as_path(),
                &["--profile", "demo", "up", "--build", "-d"],
            ),
            &repository,
        )
        .await?;
    }
    let compose_status = output(
        "docker",
        &compose_command(compose.as_path(), &["ps", "--all", "--format", "json"]),
        &repository,
    )
    .await?;
    ensure!(
        compose_status.status.success(),
        "docker compose ps failed: {}",
        String::from_utf8_lossy(&compose_status.stderr).trim()
    );
    verify_compose_state(&String::from_utf8_lossy(&compose_status.stdout))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(
            std::env::var("VERIFY_HTTP_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(20),
        ))
        .build()?;
    for (name, env_name, default_url) in stack_http_defaults() {
        let url = nonempty_env(env_name).unwrap_or_else(|| default_url.to_owned());
        check_http_endpoint(&client, name, &url).await?;
    }
    verify_compose_dependency_surfaces(&compose, &repository).await?;
    println!(
        "commerce stack verification passed: Compose state, HTTP readiness, and dependency probes"
    );
    Ok(0)
}

pub(crate) async fn observable_test(args: Vec<String>) -> anyhow::Result<i32> {
    let stack = args.first().map(String::as_str).unwrap_or("");
    let acceptance = args.get(1).map(String::as_str);
    if !matches!(stack, "rust" | "java" | "web")
        || acceptance.is_some_and(|value| value != "--acceptance")
    {
        bail!("usage: mise run test:observable -- <rust|java|web> [--acceptance]");
    }
    let invocation_id = std::env::var("CLI_INVOCATION_ID")
        .context("CLI_INVOCATION_ID is required; run through parallax invocation start")?;
    ensure!(
        std::env::var("TRACEPARENT").is_ok(),
        "TRACEPARENT is required; run through parallax invocation start"
    );
    let repository = root();
    match stack {
        "rust" => {
            status(
                "cargo",
                &[
                    "nextest".into(),
                    "run".into(),
                    "--locked".into(),
                    "--workspace".into(),
                    "--profile".into(),
                    "ci".into(),
                    "--no-tests=fail".into(),
                ],
                &repository,
            )
            .await?;
            status(
                "cargo",
                &[
                    "run".into(),
                    "--locked".into(),
                    "-p".into(),
                    "playground-cli".into(),
                    "--".into(),
                    "test-report".into(),
                    "target/nextest/ci/junit.xml".into(),
                ],
                &repository,
            )
            .await?;
        }
        "java" => {
            for service_name in ["catalog", "payment", "fulfillment"] {
                let service = repository.join("services").join(service_name);
                let wrapper = service.join("gradle/wrapper/gradle-wrapper.jar");
                ensure!(
                    wrapper.exists(),
                    "Gradle wrapper jar is missing: {}",
                    wrapper.display()
                );
                status(
                    "java",
                    &[
                        "-cp".into(),
                        wrapper.display().to_string(),
                        "org.gradle.wrapper.GradleWrapperMain".into(),
                        "--no-daemon".into(),
                        "test".into(),
                        "-Dorg.gradle.native=false".into(),
                    ],
                    &service,
                )
                .await
                .with_context(|| format!("Java observable tests failed: {service_name}"))?;
            }
        }
        "web" => {
            status(
                "bun",
                &["run".into(), "test".into()],
                &repository.join("web"),
            )
            .await?;
            let plan = web_observable_plan(acceptance == Some("--acceptance"));
            let command = plan
                .command
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>();
            if plan.clear_env.is_empty() {
                status_with_env("bun", &command, &repository.join("web"), plan.env).await?;
            } else {
                status_with_env_clearing(
                    "bun",
                    &command,
                    &repository.join("web"),
                    plan.env,
                    plan.clear_env,
                )
                .await?;
            }
        }
        _ => unreachable!(),
    }
    if acceptance == Some("--acceptance") {
        let api_url =
            nonempty_env("PARALLAX_API_URL").unwrap_or_else(|| "http://127.0.0.1:4000".to_owned());
        let summary = test_verify::verify(&api_url, &invocation_id, stack).await?;
        println!(
            "observable acceptance passed: {} trace(s), {} test attempt(s), {} application descendant(s)",
            summary.traces, summary.test_attempts, summary.app_descendants
        );
    }
    Ok(0)
}

pub(crate) async fn postgres_migrate(args: Vec<String>) -> anyhow::Result<i32> {
    let repository = root();
    let startup_timeout = positive_env("MIGRATION_TIMEOUT_SECONDS", 120)?;
    let poll_seconds = positive_env("MIGRATION_POLL_SECONDS", 1)?;
    ensure!(
        poll_seconds <= startup_timeout,
        "MIGRATION_POLL_SECONDS must not exceed MIGRATION_TIMEOUT_SECONDS: {poll_seconds}"
    );
    let statement_timeout = positive_env("MIGRATION_STATEMENT_TIMEOUT_MS", 10_000)?;
    let lock_timeout = positive_env("MIGRATION_LOCK_TIMEOUT_MS", 5_000)?;
    let connect_timeout = positive_env("PGCONNECT_TIMEOUT", 5)?;
    let postgres = PostgresEnv::new(statement_timeout, lock_timeout, connect_timeout);
    let migration_root = if let Some(path) = args.first() {
        PathBuf::from(path)
    } else {
        std::env::var("MIGRATION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repository.join("deploy/postgres/migrations"))
    };
    let mut migrations = fs::read_dir(&migration_root)
        .with_context(|| {
            format!(
                "migration directory is missing: {}",
                migration_root.display()
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect::<Vec<_>>();
    migrations.sort();
    if migrations.is_empty() {
        bail!(
            "no PostgreSQL migrations found in {}",
            migration_root.display()
        );
    }
    for migration in &migrations {
        migration_version(migration)?;
    }
    wait_for_postgres(&repository, &postgres, startup_timeout, poll_seconds).await?;
    let psql = postgres_program("psql");

    const APPLY_MIGRATION: &str = r#"
BEGIN;
SET LOCAL search_path TO public;
SELECT pg_try_advisory_xact_lock(hashtextextended('telemetry-playground:schema-migrations', 0))
    AS migration_lock_acquired \gset
\if :migration_lock_acquired
\else
    \echo migration lock is busy; refusing to wait past the verifier bound
    ROLLBACK;
    \quit 1
\endif

CREATE TABLE IF NOT EXISTS public.schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

SELECT EXISTS (
    SELECT 1
    FROM public.schema_migrations
    WHERE version = :'migration_version'
) AS migration_applied \gset

\if :migration_applied
    \echo migration :migration_version already applied
\else
    \echo applying migration :migration_version
    \ir :migration_path
    INSERT INTO public.schema_migrations (version)
    VALUES (:'migration_version');
\endif

COMMIT;
"#;
    for migration in migrations {
        let version = migration_version(&migration)?;
        let args = vec![
            "-v".into(),
            "ON_ERROR_STOP=1".into(),
            "-v".into(),
            format!("migration_path={}", migration.display()),
            "-v".into(),
            format!("migration_version={version}"),
            "-f".into(),
            "-".into(),
        ];
        status_with_input(
            &psql,
            &args,
            &repository,
            &postgres.pairs(),
            APPLY_MIGRATION,
        )
        .await
        .with_context(|| format!("migration failed: {}", migration.display()))?;
    }
    postgres_verify(Vec::new()).await
}

pub(crate) async fn postgres_verify(_args: Vec<String>) -> anyhow::Result<i32> {
    let repository = root();
    let statement_timeout = positive_env_fallback(
        "SCHEMA_STATEMENT_TIMEOUT_MS",
        "MIGRATION_STATEMENT_TIMEOUT_MS",
        10_000,
    )?;
    let lock_timeout =
        positive_env_fallback("SCHEMA_LOCK_TIMEOUT_MS", "MIGRATION_LOCK_TIMEOUT_MS", 5_000)?;
    let connect_timeout = positive_env("PGCONNECT_TIMEOUT", 5)?;
    let postgres = PostgresEnv::new(statement_timeout, lock_timeout, connect_timeout);
    let expected_tables = sql_values(
        REQUIRED_POSTGRES_TABLES
            .iter()
            .map(|table| (*table).to_owned()),
    );
    let expected_columns = REQUIRED_POSTGRES_COLUMNS
        .iter()
        .map(|(table, column)| format!("('{}','{}')", table, column))
        .collect::<Vec<_>>()
        .join(", ");
    let expected_primary_keys = sql_values(
        REQUIRED_POSTGRES_TABLES
            .iter()
            .map(|table| (*table).to_owned()),
    );
    let verify_sql = format!(
        r#"
WITH expected_tables(table_name) AS (
    VALUES {expected_tables}
),
missing_tables AS (
    SELECT COALESCE(string_agg(table_name, ', ' ORDER BY table_name), '') AS value
    FROM expected_tables
    WHERE to_regclass('public.' || table_name) IS NULL
),
expected_columns(table_name, column_name) AS (
    VALUES {expected_columns}
),
missing_columns AS (
    SELECT COALESCE(
        string_agg(
            expected_columns.table_name || '.' || expected_columns.column_name,
            ', '
            ORDER BY expected_columns.table_name, expected_columns.column_name
        ),
        ''
    ) AS value
    FROM expected_columns
    LEFT JOIN information_schema.columns actual
      ON actual.table_schema = 'public'
     AND actual.table_name = expected_columns.table_name
     AND actual.column_name = expected_columns.column_name
    WHERE actual.column_name IS NULL
),
expected_primary_keys(table_name) AS (
    VALUES {expected_primary_keys}
),
missing_primary_keys AS (
    SELECT COALESCE(string_agg(expected_primary_keys.table_name, ', ' ORDER BY expected_primary_keys.table_name), '') AS value
    FROM expected_primary_keys
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_constraint constraint_row
        JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
        JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
        WHERE schema_row.nspname = 'public'
          AND table_row.relname = expected_primary_keys.table_name
          AND constraint_row.contype = 'p'
    )
),
required_constraints AS (
    SELECT concat_ws(
        ', ',
        CASE WHEN NOT EXISTS (
            SELECT 1
            FROM pg_constraint constraint_row
            JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
            JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
            WHERE schema_row.nspname = 'public'
              AND table_row.relname = 'checkout_payment_reconciliations'
              AND constraint_row.contype = 'p'
              AND pg_get_constraintdef(constraint_row.oid) = 'PRIMARY KEY (tenant_id, request_id)'
        ) THEN 'checkout_payment_reconciliations primary key (tenant_id, request_id)' END,
        CASE WHEN NOT EXISTS (
            SELECT 1
            FROM pg_constraint constraint_row
            JOIN pg_class table_row ON table_row.oid = constraint_row.conrelid
            JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
            WHERE schema_row.nspname = 'public'
              AND table_row.relname = 'fulfillment_processed_events'
              AND constraint_row.contype = 'p'
              AND pg_get_constraintdef(constraint_row.oid) = 'PRIMARY KEY (tenant_id, consumer_name, event_key)'
        ) THEN 'fulfillment_processed_events primary key (tenant_id, consumer_name, event_key)' END
    ) AS value
)
SELECT concat_ws(
    E'\n',
    CASE WHEN missing_tables.value = '' THEN NULL ELSE 'required table(s) missing: ' || missing_tables.value END,
    CASE WHEN missing_columns.value = '' THEN NULL ELSE 'required column(s) missing: ' || missing_columns.value END,
    CASE WHEN missing_primary_keys.value = '' THEN NULL ELSE 'required primary key(s) missing: ' || missing_primary_keys.value END,
    CASE WHEN required_constraints.value IS NULL OR required_constraints.value = '' THEN NULL ELSE 'required constraint(s) missing: ' || required_constraints.value END
)
FROM missing_tables, missing_columns, missing_primary_keys, required_constraints;
"#
    );
    let psql = postgres_program("psql");
    let result = output_with_env(
        &psql,
        &[
            "-Atq".into(),
            "-v".into(),
            "ON_ERROR_STOP=1".into(),
            "-c".into(),
            verify_sql,
        ],
        &repository,
        &postgres.pairs(),
    )
    .await?;
    ensure!(
        result.status.success(),
        "PostgreSQL schema verification command failed: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    let missing = String::from_utf8_lossy(&result.stdout).trim().to_owned();
    ensure!(
        missing.is_empty(),
        "PostgreSQL schema is incomplete:\n{missing}"
    );
    println!("PostgreSQL schema verification passed");
    Ok(0)
}

pub(crate) async fn postgres_idempotence(_args: Vec<String>) -> anyhow::Result<i32> {
    let repository = root();
    let compose = repository.join("deploy/docker-compose.yml");
    let project = format!(
        "telemetry-playground-clean-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let compose_args = |command: &[&str]| {
        let mut args = vec![
            "compose".into(),
            "-p".into(),
            project.clone(),
            "-f".into(),
            compose.display().to_string(),
        ];
        args.extend(command.iter().map(|value| (*value).to_owned()));
        args
    };
    let result = async {
        status(
            "docker",
            &compose_args(&["up", "-d", "postgres"]),
            &repository,
        )
        .await?;
        status(
            "docker",
            &compose_args(&["run", "--rm", "--build", "postgres-migrate"]),
            &repository,
        )
        .await?;
        let read_state = || async {
            let output = output(
                "docker",
                &compose_args(&[
                    "exec",
                    "-T",
                    "postgres",
                    "psql",
                    "-U",
                    "postgres",
                    "-d",
                    "playground",
                    "-Atqc",
                    "SELECT string_agg(version || '=' || applied_at::text, ',' ORDER BY version) FROM public.schema_migrations;",
                ]),
                &repository,
            )
            .await?;
            ensure!(
                output.status.success(),
                "schema state query failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            Ok::<_, anyhow::Error>(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let before = read_state().await?;
        status(
            "docker",
            &compose_args(&["run", "--rm", "postgres-migrate"]),
            &repository,
        )
        .await?;
        let after = read_state().await?;
        ensure!(
            before == after,
            "idempotent migration proof changed schema_migrations: before={before:?} after={after:?}"
        );
        println!("fresh Compose migration is idempotent: {project}");
        Ok::<_, anyhow::Error>(0)
    }
    .await;
    let cleanup = status(
        "docker",
        &compose_args(&["down", "-v", "--remove-orphans"]),
        &repository,
    )
    .await;
    if let Err(error) = cleanup {
        if result.is_ok() {
            return Err(error);
        }
        eprintln!("warning: failed to clean Compose project {project}: {error:#}");
    }
    result
}

pub(crate) async fn webhook_listener(args: Vec<String>) -> anyhow::Result<i32> {
    let port = args
        .first()
        .map(String::as_str)
        .unwrap_or("9099")
        .parse::<u16>()
        .context("webhook listener port must be a valid u16")?;
    async fn handle(request: Request<Body>) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 10 * 1024 * 1024).await.unwrap_or_default();
        println!(
            "\n── webhook {:?} {} {}",
            std::time::SystemTime::now(),
            parts.method,
            parts.uri
        );
        for (key, value) in &parts.headers {
            println!("   {key}: {value:?}");
        }
        match serde_json::from_slice::<Value>(&body) {
            Ok(json) => println!(
                "{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            ),
            Err(_) => println!("{}", String::from_utf8_lossy(&body)),
        }
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("ok"))
            .expect("static response is valid")
    }
    let router = Router::new().route("/", post(handle)).fallback(handle);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("webhook-listener: listening on http://127.0.0.1:{port} (Ctrl-C to stop)");
    serve(listener, router).await?;
    Ok(0)
}

pub(crate) async fn demo_stack(args: Vec<String>) -> anyhow::Result<i32> {
    let repository = root();
    let mut command_args = vec![
        "compose".into(),
        "-f".into(),
        repository
            .join("deploy/docker-compose.yml")
            .display()
            .to_string(),
        "--profile".into(),
        "demo".into(),
        "up".into(),
        "--build".into(),
        "-d".into(),
    ];
    command_args.extend(args);
    status("docker", &command_args, &repository).await?;
    println!("playground stack is running; inspect it with `mise tasks ls --sort name`");
    Ok(0)
}

pub(crate) async fn demo_fresh(args: Vec<String>) -> anyhow::Result<i32> {
    ensure!(
        args.len() == 1 && args[0] == "--yes",
        "demo:fresh destroys local Compose volumes; pass --yes explicitly"
    );
    let repository = root();
    let compose = repository.join("deploy/docker-compose.yml");
    let down = vec![
        "compose".into(),
        "-f".into(),
        compose.display().to_string(),
        "--profile".into(),
        "demo".into(),
        "down".into(),
        "-v".into(),
        "--remove-orphans".into(),
    ];
    status("docker", &down, &repository).await?;
    let up = vec![
        "compose".into(),
        "-f".into(),
        compose.display().to_string(),
        "--profile".into(),
        "demo".into(),
        "up".into(),
        "--build".into(),
        "-d".into(),
    ];
    status("docker", &up, &repository).await?;
    println!("fresh playground stack is running with new Compose volumes");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        REQUIRED_COMPLETED_COMPOSE_SERVICES, REQUIRED_RUNNING_COMPOSE_SERVICES, is_semantic_name,
        stack_http_defaults, validate_traceparent, verify_compose_state, web_observable_plan,
    };

    #[test]
    fn semantic_name_contract_accepts_lower_snake_case_segments() {
        assert!(is_semantic_name("jvm:memory_pressure"));
        assert!(is_semantic_name("feature_flags:checkout_variants"));
        assert!(is_semantic_name("alerts:p95_breach"));
    }

    #[test]
    fn semantic_name_contract_rejects_opaque_or_malformed_names() {
        for name in [
            "b20",
            "jvm:memory-pressure",
            "JVM:memory_pressure",
            "jvm:_memory_pressure",
            "jvm:memory__pressure",
            "jvm:memory_pressure_",
            "jvm:memory_pressure:extra",
        ] {
            assert!(!is_semantic_name(name), "accepted invalid name: {name}");
        }
    }

    #[test]
    fn traceparent_validation_extracts_trace_id() {
        assert_eq!(
            validate_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
                .expect("valid traceparent"),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }

    #[test]
    fn traceparent_validation_rejects_zero_ids() {
        assert!(
            validate_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01")
                .is_err()
        );
        assert!(
            validate_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01")
                .is_err()
        );
    }

    #[test]
    fn acceptance_web_plan_uses_real_compose_and_clears_mock_mode() {
        let plan = web_observable_plan(true);
        assert_eq!(plan.command, &["run", "e2e:compose"]);
        assert_eq!(plan.env, &[("PLAYGROUND_COMPOSE_E2E", "1")]);
        assert_eq!(plan.clear_env, &["PLAYGROUND_MOCK_E2E"]);

        let mock_plan = web_observable_plan(false);
        assert_eq!(
            mock_plan.command[0],
            "./node_modules/@playwright/test/cli.js"
        );
        assert_eq!(mock_plan.env, &[("PLAYGROUND_MOCK_E2E", "1")]);
    }

    #[test]
    fn stack_http_defaults_use_compose_readiness_contracts() {
        let defaults = stack_http_defaults();
        let endpoint = |name: &str| {
            defaults
                .iter()
                .find(|(service, _, _)| *service == name)
                .map(|(_, _, url)| *url)
                .expect("stack endpoint")
        };
        assert_eq!(endpoint("Checkout"), "http://127.0.0.1:8088/readyz");
        assert_eq!(
            endpoint("Catalog"),
            "http://127.0.0.1:8080/actuator/health/readiness"
        );
        assert_eq!(endpoint("Recommendation"), "http://127.0.0.1:8090/readyz");
        assert_eq!(
            endpoint("Storefront analytics"),
            "http://127.0.0.1:8094/analytics/readyz"
        );
    }

    #[test]
    fn compose_state_requires_all_services_and_successful_jobs() {
        let mut rows = REQUIRED_RUNNING_COMPOSE_SERVICES
            .iter()
            .map(|service| {
                json!({
                    "Service": service,
                    "State": "running",
                    "Health": "healthy",
                    "ExitCode": 0
                })
                .to_string()
            })
            .collect::<Vec<_>>();
        rows.extend(REQUIRED_COMPLETED_COMPOSE_SERVICES.iter().map(|service| {
            json!({
                "Service": service,
                "State": "exited",
                "Health": "",
                "ExitCode": 0
            })
            .to_string()
        }));
        let healthy = rows.join("\n");
        assert!(verify_compose_state(&healthy).is_ok());

        let unhealthy = healthy.replacen("\"Health\":\"healthy\"", "\"Health\":\"starting\"", 1);
        assert!(verify_compose_state(&unhealthy).is_err());

        let mut failed_rows = rows;
        failed_rows[REQUIRED_RUNNING_COMPOSE_SERVICES.len()] = json!({
            "Service": REQUIRED_COMPLETED_COMPOSE_SERVICES[0],
            "State": "exited",
            "Health": "",
            "ExitCode": 1
        })
        .to_string();
        assert!(verify_compose_state(&failed_rows.join("\n")).is_err());
    }
}

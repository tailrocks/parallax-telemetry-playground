//! Playground driver CLI — short-lived process producing run-scoped telemetry.
//!   playground            drive the checkout flow (A1/A12)
//!   playground cron       a scheduled job with weighted outcomes (B17):
//!                         ~90% success, ~5% failure (nonzero exit),
//!                         ~5% "stuck" (long sleep → missed check-in)
//!   playground daemon     host CLI → daemon → child/container → agent sim
//!   playground enter      child/container side of the execution-stack sim
//! Flushes telemetry on exit (short-lived discipline).

use tokio::process::Command;
use tracing::Instrument;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("playground-cli")?;
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let rest = args.collect::<Vec<_>>();
    let result = match mode.as_str() {
        "cron" => cron().await,
        "daemon" => daemon(rest).await,
        "enter" => enter(rest).await,
        _ => drive().await,
    };
    let code = match result {
        Ok(code) => code,
        Err(err) => {
            playground_telemetry::mark_span_error("cli_error");
            tracing::error!(error = %err, "cli failed");
            1
        }
    };
    telemetry.shutdown(); // flush before exit
    std::process::exit(code);
}

#[tracing::instrument(fields(otel.kind = "client"))]
async fn drive() -> anyhow::Result<i32> {
    let base = std::env::var("CHECKOUT_URL").unwrap_or_else(|_| "http://localhost:8088".into());
    let url = format!("{base}/checkout?sku=WIDGET-1&quantity=3");
    let body = playground_telemetry::traced_get(&url).await?.text().await?;
    tracing::info!(%url, "drove checkout");
    println!("{body}");
    Ok(0)
}

async fn daemon(args: Vec<String>) -> anyhow::Result<i32> {
    let session = option_value(&args, "--session").unwrap_or_else(default_session_id);
    let run_id = run_id(&session);
    let orphan = flag_present(&args, "--orphan");
    let span = tracing::info_span!(
        "host_cli",
        otel.kind = "client",
        "cli.command" = "playground daemon",
        "parallax.session.id" = %session,
        "parallax.run.id" = %run_id,
        orphan
    );
    async move { daemon_session(session, run_id, orphan).await }
        .instrument(span)
        .await
}

async fn daemon_session(session: String, run_id: String, orphan: bool) -> anyhow::Result<i32> {
    let span = tracing::info_span!(
        "daemon_session",
        otel.kind = "internal",
        "parallax.execution.layer" = "daemon",
        "parallax.session.id" = %session,
        "parallax.run.id" = %run_id,
        orphan
    );
    async move {
        let mut child = Command::new(std::env::current_exe()?);
        child.arg("enter").arg("--session").arg(&session);
        child.env("PARALLAX_RUN_ID", &run_id);
        child.env(
            "OTEL_RESOURCE_ATTRIBUTES",
            resource_attrs_with_run_id(&run_id),
        );
        if orphan {
            child.arg("--orphan");
            child.env_remove("TRACEPARENT");
            child.env_remove("TRACESTATE");
            child.env_remove("BAGGAGE");
        } else {
            for (key, value) in playground_telemetry::current_context_env() {
                child.env(key, value);
            }
            child.env(
                "BAGGAGE",
                format!("parallax.session.id={session},parallax.run.id={run_id}"),
            );
        }

        tracing::info!(%session, %run_id, orphan, "spawning execution child");
        let status = child.status().await?;
        let code = status.code().unwrap_or(1);
        if !status.success() {
            playground_telemetry::mark_span_error("child_exit");
            tracing::error!(exit_code = code, "execution child failed");
        }
        Ok(code)
    }
    .instrument(span)
    .await
}

async fn enter(args: Vec<String>) -> anyhow::Result<i32> {
    let session = option_value(&args, "--session").unwrap_or_else(default_session_id);
    let run_id = run_id(&session);
    let orphan = flag_present(&args, "--orphan");
    let span = if orphan {
        tracing::info_span!(
            "container_session",
            otel.kind = "client",
            "url.full" = "container://agent",
            "parallax.execution.layer" = "container",
            "parallax.session.id" = %session,
            "parallax.run.id" = %run_id,
            orphan
        )
    } else {
        tracing::info_span!(
            "container_session",
            otel.kind = "internal",
            "parallax.execution.layer" = "container",
            "parallax.session.id" = %session,
            "parallax.run.id" = %run_id,
            orphan
        )
    };
    playground_telemetry::set_parent_from_env(&span);
    async move {
        tracing::info!(%session, %run_id, orphan, "entered simulated container");
        invoke_agent(&session, &run_id).await;
        Ok(0)
    }
    .instrument(span)
    .await
}

async fn invoke_agent(session: &str, run_id: &str) {
    let span = tracing::info_span!(
        "invoke_agent",
        otel.kind = "internal",
        "gen_ai.operation.name" = "invoke_agent",
        "parallax.agent.id" = "demo-agent",
        "parallax.session.id" = %session,
        "parallax.run.id" = %run_id
    );
    async move {
        tracing::info!("agent invocation started");
        execute_tool("inspect_repo", "rg --files", false).await;
        execute_tool("shell_command", "false", true).await;
        tracing::info!("agent invocation finished");
    }
    .instrument(span)
    .await
}

async fn execute_tool(tool: &'static str, command: &'static str, fail: bool) {
    let span = tracing::info_span!(
        "execute_tool",
        otel.kind = "internal",
        "gen_ai.operation.name" = "execute_tool",
        "tool.name" = %tool,
        "shell.command" = %command
    );
    async move {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        if fail {
            playground_telemetry::mark_span_error("command_exit");
            tracing::error!(exit_code = 2, %tool, %command, "tool command failed");
        } else {
            tracing::info!(%tool, %command, "tool command succeeded");
        }
    }
    .instrument(span)
    .await
}

/// B17: weighted cron outcome. Deterministic source (process nanos) avoids a rand
/// dep; bucket 0–89 ok, 90–94 fail, 95–99 stuck.
#[tracing::instrument(fields(otel.kind = "internal"))]
async fn cron() -> anyhow::Result<i32> {
    let bucket = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        % 100) as u8;
    match bucket {
        0..=89 => {
            tracing::info!(bucket, "cron job succeeded");
            Ok(0)
        }
        90..=94 => {
            playground_telemetry::mark_span_error("nonzero_exit");
            tracing::error!(bucket, "cron job failed");
            Ok(1)
        }
        _ => {
            tracing::warn!(bucket, "cron job stuck — long-running (missed check-in)");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(0)
        }
    }
}

fn flag_present(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn option_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn default_session_id() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("exec-stack-{seconds}-{}", std::process::id())
}

fn run_id(session: &str) -> String {
    std::env::var("PARALLAX_RUN_ID").unwrap_or_else(|_| session.to_string())
}

fn resource_attrs_with_run_id(run_id: &str) -> String {
    let existing = std::env::var("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default();
    if existing
        .split(',')
        .filter_map(|item| item.split_once('='))
        .any(|(key, _)| key.trim() == "parallax.run.id")
    {
        return existing;
    }
    if existing.trim().is_empty() {
        format!("parallax.run.id={run_id}")
    } else {
        format!("{existing},parallax.run.id={run_id}")
    }
}

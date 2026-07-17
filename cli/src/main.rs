//! Playground driver CLI — short-lived process producing invocation-scoped
//! telemetry in the neutral CLI contract (`cli.invocation.id`, `app.mode`,
//! `outcome`; plan 158).
//!   playground            drive the checkout flow (A1/A12), one_shot
//!   playground cron       a scheduled job with weighted outcomes (B17),
//!                         one_shot per firing
//!   playground daemon     host CLI → daemon (+ background cycles) →
//!                         capsule child → agent sim
//!   playground enter      capsule side of the execution-stack sim
//!   playground console    scripted interactive TUI session (sessions,
//!                         screens, ui.action roots)
//! Flushes telemetry on exit (short-lived discipline).

mod console_sim;
mod shapes;
mod test_report;
mod test_verify;

use std::path::Path;
use std::process::Command as ProcessCommand;

use tokio::process::Command;
use tracing::Instrument;

use playground_telemetry::invocation;
use playground_telemetry::semconv;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let rest = args.collect::<Vec<_>>();
    if mode == "cron" && rest.first().map(String::as_str) == Some("missed") {
        println!("cron missed: no process telemetry emitted");
        return Ok(());
    }

    // The report converter is useful as a local JUnit reconciliation tool as
    // well as a live OTLP producer. Avoid the SDK's implicit localhost exporter
    // when no collector was requested; `parallax invocation start` supplies
    // the endpoint for the observable path.
    let telemetry = if matches!(mode.as_str(), "test-report" | "test-verify")
        && std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .ok()
            .is_none_or(|endpoint| endpoint.trim().is_empty())
    {
        None
    } else {
        Some(playground_telemetry::init("playground-cli")?)
    };
    let command_name = command_name(&mode);
    let app_mode = app_mode(&mode);
    let invocation_id = invocation::invocation_id();
    let root = tracing::info_span!(
        "cli.command",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        cli.invocation.id = %invocation_id,
        cli.command.name = %command_name,
        app.mode = %app_mode,
        outcome = tracing::field::Empty,
        process.exit.code = tracing::field::Empty,
    );
    let result = async {
        match mode.as_str() {
            "test-report" => test_report_command(&rest),
            "test-verify" => test_verify_command(&rest).await,
            "cron" => cron(rest).await,
            "daemon" => daemon(rest).await,
            "enter" => enter(rest).await,
            "console" => console_sim::run(rest).await,
            "shapes" => shapes::run(rest).await,
            _ => drive().await,
        }
    }
    .instrument(root.clone())
    .await;
    let code = match result {
        Ok(code) => code,
        Err(err) => {
            playground_telemetry::mark_span_error("cli_error");
            tracing::error!(error = %err, "cli failed");
            // tracing may have no subscriber (telemetry off) — always reach the
            // operator's terminal too.
            eprintln!("error: {err:#}");
            1
        }
    };
    root.record("outcome", outcome_for_exit(code));
    root.record("process.exit.code", i64::from(code));
    drop(root);
    if let Some(telemetry) = telemetry {
        telemetry.shutdown(); // flush before exit
    }
    std::process::exit(code);
}

/// Bounded dotted command-registry names (neutral contract decision 3).
fn command_name(mode: &str) -> &'static str {
    match mode {
        "cron" => "playground.cron",
        "daemon" => "playground.daemon",
        "enter" => "playground.enter",
        "console" => "playground.console",
        "shapes" => "playground.shapes",
        "test-report" => "playground.test.report",
        "test-verify" => "playground.test.verify",
        _ => "playground.drive",
    }
}

/// `app.mode` per mode: each cron firing is an invocation (one_shot); the
/// capsule child layer reports `capsule`.
fn app_mode(mode: &str) -> &'static str {
    match mode {
        "daemon" => semconv::APP_MODE_DAEMON,
        "enter" => semconv::APP_MODE_CAPSULE,
        "console" => semconv::APP_MODE_INTERACTIVE,
        _ => semconv::APP_MODE_ONE_SHOT,
    }
}

fn outcome_for_exit(code: i32) -> &'static str {
    if code == 0 {
        semconv::OUTCOME_SUCCESS
    } else {
        semconv::OUTCOME_FAILURE
    }
}

async fn test_verify_command(args: &[String]) -> anyhow::Result<i32> {
    let [invocation_id, stack, rest @ ..] = args else {
        return Err(anyhow::anyhow!(
            "usage: playground test-verify <invocation-id> <rust|java|web> [parallax-api-url]"
        ));
    };
    let api_url = rest
        .first()
        .map(String::as_str)
        .unwrap_or("http://127.0.0.1:4000");
    let summary = test_verify::verify(api_url, invocation_id, stack).await?;
    println!(
        "verified {stack} observable invocation {invocation_id}: {} traces, {} test attempts, {} app descendants",
        summary.traces, summary.test_attempts, summary.app_descendants
    );
    Ok(0)
}

fn test_report_command(args: &[String]) -> anyhow::Result<i32> {
    let Some(path) = args.first() else {
        return Err(anyhow::anyhow!("usage: playground test-report <junit.xml>"));
    };
    let summary = test_report::emit(Path::new(path))?;
    println!(
        "reported {} test attempts ({} passed, {} failed, {} errors)",
        summary.total, summary.passed, summary.failed, summary.errors
    );
    Ok(if summary.final_failures == 0 { 0 } else { 1 })
}

#[tracing::instrument(fields(otel.kind = semconv::SPAN_KIND_CLIENT))]
async fn drive() -> anyhow::Result<i32> {
    let base = std::env::var("CHECKOUT_URL").unwrap_or_else(|_| "http://localhost:8088".into());
    let url = format!("{base}/checkout?sku=WIDGET-1&quantity=3");
    let body = playground_telemetry::traced_get(&url).await?.text().await?;
    // Log records don't inherit span attributes; stamp the invocation id so
    // invocation-scoped log queries can find this line.
    tracing::info!(
        %url,
        cli.invocation.id = %invocation::invocation_id(),
        "drove checkout"
    );
    println!("{body}");
    Ok(0)
}

async fn daemon(args: Vec<String>) -> anyhow::Result<i32> {
    let session = option_value(&args, "--session").unwrap_or_else(invocation::new_session_id);
    let invocation_id = invocation::invocation_id().to_owned();
    let orphan = flag_present(&args, "--orphan");
    let span = tracing::info_span!(
        "host_cli",
        otel.kind = semconv::SPAN_KIND_CLIENT,
        cli.invocation.id = %invocation_id,
        session.id = %session,
        orphan
    );
    async move { daemon_session(session, invocation_id, orphan).await }
        .instrument(span)
        .await
}

async fn daemon_session(
    session: String,
    invocation_id: String,
    orphan: bool,
) -> anyhow::Result<i32> {
    let span = tracing::info_span!(
        "daemon_session",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        app.mode = semconv::APP_MODE_DAEMON,
        session.id = %session,
        orphan
    );
    async move {
        // Substantive periodic daemon work: two named reconciliation cycles
        // (neutral contract decision 7) emitted while the daemon runs.
        background_cycle(semconv::BACKGROUND_CYCLE_QUEUE_HEALTH, false).await;
        background_cycle(semconv::BACKGROUND_CYCLE_PRICE_REFRESH, true).await;

        let carrier = playground_telemetry::current_context_env();
        let child = execution_child_command(
            &std::env::current_exe()?,
            &session,
            &invocation_id,
            orphan,
            &carrier,
            std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok().as_deref(),
        );
        let mut child = Command::from(child);

        tracing::info!(%session, %invocation_id, orphan, "spawning execution child");
        let status = child.status().await?;
        // Acceptance runs observe the daemon while alive (`--hold-seconds`):
        // emit a queue-health cycle every 5 s so activity stays visible.
        if let Some(hold) = std::env::var("PLAYGROUND_DAEMON_HOLD_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(hold);
            while std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                background_cycle(semconv::BACKGROUND_CYCLE_QUEUE_HEALTH, false).await;
            }
        }
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

/// One `background.cycle` root span (a fresh trace: cycles are periodic
/// daemon work, not part of the spawn trace).
async fn background_cycle(name: &'static str, fail: bool) {
    let invocation_id = invocation::invocation_id();
    let span = tracing::info_span!(
        parent: None,
        "background.cycle",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        cli.invocation.id = %invocation_id,
        background.cycle.name = %name,
        outcome = if fail { semconv::OUTCOME_FAILURE } else { semconv::OUTCOME_SUCCESS },
    );
    async move {
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        if fail {
            playground_telemetry::mark_span_error("cycle_failure");
            tracing::error!(cycle = %name, "background cycle failed");
        } else {
            tracing::info!(cycle = %name, "background cycle completed");
        }
    }
    .instrument(span)
    .await
}

fn execution_child_command(
    executable: &Path,
    session: &str,
    invocation_id: &str,
    orphan: bool,
    carrier: &[(String, String)],
    resource_attributes: Option<&str>,
) -> ProcessCommand {
    let mut child = ProcessCommand::new(executable);
    child.arg("enter").arg("--session").arg(session);
    child.env(invocation::INVOCATION_ENV, invocation_id);
    child.env(
        "OTEL_RESOURCE_ATTRIBUTES",
        invocation::resource_attrs_with_invocation_id(resource_attributes, invocation_id),
    );
    if orphan {
        child.arg("--orphan");
        child.env_remove("TRACEPARENT");
        child.env_remove("TRACESTATE");
        child.env_remove("BAGGAGE");
    } else {
        for (key, value) in carrier {
            child.env(key, value);
        }
        child.env(
            "BAGGAGE",
            format!(
                "{}={session},{}={invocation_id}",
                semconv::SESSION_ID,
                semconv::CLI_INVOCATION_ID
            ),
        );
    }
    child
}

async fn enter(args: Vec<String>) -> anyhow::Result<i32> {
    let session = option_value(&args, "--session").unwrap_or_else(invocation::new_session_id);
    let invocation_id = invocation::invocation_id().to_owned();
    let orphan = flag_present(&args, "--orphan");
    let span = if orphan {
        tracing::info_span!(
            "container_session",
            otel.kind = semconv::SPAN_KIND_CLIENT,
            url.full = "container://agent",
            app.mode = semconv::APP_MODE_CAPSULE,
            cli.invocation.id = %invocation_id,
            session.id = %session,
            orphan
        )
    } else {
        tracing::info_span!(
            "container_session",
            otel.kind = semconv::SPAN_KIND_INTERNAL,
            app.mode = semconv::APP_MODE_CAPSULE,
            cli.invocation.id = %invocation_id,
            session.id = %session,
            orphan
        )
    };
    playground_telemetry::set_parent_from_env(&span);
    async move {
        tracing::info!(%session, %invocation_id, orphan, "entered simulated capsule");
        invoke_agent(&session).await;
        Ok(0)
    }
    .instrument(span)
    .await
}

async fn invoke_agent(session: &str) {
    let conversation_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "invoke_agent",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        gen_ai.operation.name = "invoke_agent",
        gen_ai.agent.name = "claude",
        gen_ai.provider.name = "anthropic",
        gen_ai.conversation.id = %conversation_id,
        session.id = %session,
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
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        gen_ai.operation.name = "execute_tool",
        tool.name = %tool,
        shell.command = %command
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

#[derive(Debug, Clone, Copy)]
enum CronOutcome {
    Ok,
    Fail,
    Stuck,
}

impl CronOutcome {
    /// Generic bounded `outcome` (decision 5): stuck check-ins time out.
    fn as_outcome(self) -> &'static str {
        match self {
            Self::Ok => semconv::OUTCOME_SUCCESS,
            Self::Fail => semconv::OUTCOME_FAILURE,
            Self::Stuck => semconv::OUTCOME_TIMEOUT,
        }
    }
}

/// B17: weighted cron outcome. Deterministic source (process nanos) avoids a rand
/// dep; bucket 0-89 ok, 90-94 fail, 95-99 stuck.
async fn cron(args: Vec<String>) -> anyhow::Result<i32> {
    let mode = args.first().map(String::as_str).unwrap_or("weighted");
    match mode {
        "ok" => cron_once(CronOutcome::Ok, 0).await,
        "fail" => cron_once(CronOutcome::Fail, 0).await,
        "stuck" => cron_once(CronOutcome::Stuck, 0).await,
        "duplicate" => {
            let first = cron_once(CronOutcome::Ok, 1).await?;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let second = cron_once(CronOutcome::Ok, 2).await?;
            Ok(first.max(second))
        }
        "missed" => Ok(0),
        "weighted" | "" => cron_once(weighted_cron_outcome(), 0).await,
        other => Err(anyhow::anyhow!("unknown cron mode: {other}")),
    }
}

async fn cron_once(outcome: CronOutcome, duplicate_ordinal: i64) -> anyhow::Result<i32> {
    let invocation_id = invocation::invocation_id();
    let span = tracing::info_span!(
        "cron_job",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        cli.invocation.id = %invocation_id,
        "cron.job.name" = "playground-report",
        "cron.schedule" = "*/1 * * * *",
        outcome = outcome.as_outcome(),
        "cron.duplicate.ordinal" = duplicate_ordinal
    );
    async move {
        match outcome {
            CronOutcome::Ok => {
                tracing::info!(
                    cli.invocation.id = %invocation::invocation_id(),
                    "cron job succeeded"
                );
                Ok(0)
            }
            CronOutcome::Fail => {
                playground_telemetry::mark_span_error("nonzero_exit");
                tracing::error!(
                    cli.invocation.id = %invocation::invocation_id(),
                    "cron job failed"
                );
                Ok(1)
            }
            CronOutcome::Stuck => {
                tracing::warn!("cron job stuck: long-running check-in");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                Ok(0)
            }
        }
    }
    .instrument(span)
    .await
}

fn weighted_cron_outcome() -> CronOutcome {
    let bucket = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        % 100) as u8;
    match bucket {
        0..=89 => CronOutcome::Ok,
        90..=94 => CronOutcome::Fail,
        _ => CronOutcome::Stuck,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::Path;

    use super::{
        CronOutcome, app_mode, command_name, cron_once, execution_child_command, outcome_for_exit,
    };
    use playground_telemetry::semconv;

    #[tokio::test]
    async fn cron_outcomes_preserve_process_exit_contract() {
        assert_eq!(cron_once(CronOutcome::Ok, 0).await.expect("ok cron"), 0);
        assert_eq!(cron_once(CronOutcome::Fail, 0).await.expect("fail cron"), 1);
    }

    #[test]
    fn every_mode_maps_to_a_bounded_command_and_app_mode() {
        let cases = [
            ("", "playground.drive", semconv::APP_MODE_ONE_SHOT),
            ("drive", "playground.drive", semconv::APP_MODE_ONE_SHOT),
            ("cron", "playground.cron", semconv::APP_MODE_ONE_SHOT),
            ("daemon", "playground.daemon", semconv::APP_MODE_DAEMON),
            ("enter", "playground.enter", semconv::APP_MODE_CAPSULE),
            (
                "console",
                "playground.console",
                semconv::APP_MODE_INTERACTIVE,
            ),
            (
                "test-report",
                "playground.test.report",
                semconv::APP_MODE_ONE_SHOT,
            ),
            (
                "test-verify",
                "playground.test.verify",
                semconv::APP_MODE_ONE_SHOT,
            ),
        ];
        for (mode, command, app) in cases {
            assert_eq!(command_name(mode), command, "{mode}");
            assert_eq!(app_mode(mode), app, "{mode}");
        }
    }

    #[test]
    fn exit_codes_map_to_the_bounded_outcome() {
        assert_eq!(outcome_for_exit(0), semconv::OUTCOME_SUCCESS);
        assert_eq!(outcome_for_exit(1), semconv::OUTCOME_FAILURE);
    }

    #[test]
    fn execution_child_propagates_or_removes_the_process_carrier() {
        let carrier = vec![
            ("TRACEPARENT".to_owned(), "00-abc-def-01".to_owned()),
            ("TRACESTATE".to_owned(), "vendor=value".to_owned()),
        ];
        let linked = execution_child_command(
            Path::new("playground"),
            "session-a",
            "inv-a",
            false,
            &carrier,
            Some("service.name=cli"),
        );
        let linked_env = linked
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<HashMap<OsString, Option<OsString>>>();
        assert_eq!(
            linked_env.get(&OsString::from("TRACEPARENT")),
            Some(&Some(OsString::from("00-abc-def-01")))
        );
        assert_eq!(
            linked_env.get(&OsString::from("CLI_INVOCATION_ID")),
            Some(&Some(OsString::from("inv-a")))
        );
        assert_eq!(
            linked_env.get(&OsString::from("OTEL_RESOURCE_ATTRIBUTES")),
            Some(&Some(OsString::from(
                "service.name=cli,cli.invocation.id=inv-a"
            )))
        );
        assert_eq!(
            linked_env.get(&OsString::from("BAGGAGE")),
            Some(&Some(OsString::from(
                "session.id=session-a,cli.invocation.id=inv-a"
            )))
        );

        let orphan = execution_child_command(
            Path::new("playground"),
            "session-a",
            "inv-a",
            true,
            &carrier,
            None,
        );
        let orphan_env = orphan
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<HashMap<OsString, Option<OsString>>>();
        assert_eq!(orphan_env.get(&OsString::from("TRACEPARENT")), Some(&None));
        assert_eq!(orphan_env.get(&OsString::from("TRACESTATE")), Some(&None));
        assert_eq!(orphan_env.get(&OsString::from("BAGGAGE")), Some(&None));
    }

    fn w4_acceptance_attempt() -> u32 {
        if std::env::var("PLAYGROUND_TEST_FLAKY_FIXTURE").as_deref() != Ok("1") {
            return 2;
        }
        std::env::var("NEXTEST_ATTEMPT")
            .expect("W4 acceptance fixtures require cargo-nextest")
            .parse()
            .expect("NEXTEST_ATTEMPT must be a positive integer")
    }

    #[test]
    fn w4_assertion_failure_passes_on_retry() {
        assert!(
            w4_acceptance_attempt() > 1,
            "intentional first-attempt assertion failure"
        );
    }

    #[test]
    fn w4_harness_error_passes_on_retry() {
        if w4_acceptance_attempt() == 1 {
            std::process::abort();
        }
    }
}

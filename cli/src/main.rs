//! Playground driver CLI — short-lived process producing run-scoped telemetry.
//!   playground            drive the checkout flow (A1/A12)
//!   playground cron       a scheduled job with weighted outcomes (B17):
//!                         ~90% success, ~5% failure (nonzero exit),
//!                         ~5% "stuck" (long sleep → missed check-in)
//!   playground daemon     host CLI → daemon → child/container → agent sim
//!   playground enter      child/container side of the execution-stack sim
//! Flushes telemetry on exit (short-lived discipline).

mod test_report;

use std::path::Path;

use tokio::process::Command;
use tracing::Instrument;

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

    let telemetry = playground_telemetry::init("playground-cli")?;
    let result = match mode.as_str() {
        "test-report" => test_report_command(&rest),
        "cron" => cron(rest).await,
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

fn test_report_command(args: &[String]) -> anyhow::Result<i32> {
    let Some(path) = args.first() else {
        return Err(anyhow::anyhow!("usage: playground test-report <junit.xml>"));
    };
    let summary = test_report::emit(Path::new(path))?;
    println!(
        "reported {} test cases ({} passed, {} failed, {} errors)",
        summary.total, summary.passed, summary.failed, summary.errors
    );
    Ok(if summary.failed + summary.errors == 0 {
        0
    } else {
        1
    })
}

#[tracing::instrument(fields(otel.kind = semconv::SPAN_KIND_CLIENT))]
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
        otel.kind = semconv::SPAN_KIND_CLIENT,
        cli.command = "playground daemon",
        parallax.session.id = %session,
        parallax.run.id = %run_id,
        orphan
    );
    async move { daemon_session(session, run_id, orphan).await }
        .instrument(span)
        .await
}

async fn daemon_session(session: String, run_id: String, orphan: bool) -> anyhow::Result<i32> {
    let span = tracing::info_span!(
        "daemon_session",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        parallax.execution.layer = "daemon",
        parallax.session.id = %session,
        parallax.run.id = %run_id,
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
                format!(
                    "{}={session},{}={run_id}",
                    semconv::PARALLAX_SESSION_ID,
                    semconv::PARALLAX_RUN_ID
                ),
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
            otel.kind = semconv::SPAN_KIND_CLIENT,
            url.full = "container://agent",
            parallax.execution.layer = "container",
            parallax.session.id = %session,
            parallax.run.id = %run_id,
            orphan
        )
    } else {
        tracing::info_span!(
            "container_session",
            otel.kind = semconv::SPAN_KIND_INTERNAL,
            parallax.execution.layer = "container",
            parallax.session.id = %session,
            parallax.run.id = %run_id,
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
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        gen_ai.operation.name = "invoke_agent",
        parallax.agent.id = "demo-agent",
        parallax.session.id = %session,
        parallax.run.id = %run_id
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
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Fail => "fail",
            Self::Stuck => "stuck",
        }
    }
}

/// B17: weighted cron outcome. Deterministic source (process nanos) avoids a rand
/// dep; bucket 0-89 ok, 90-94 fail, 95-99 stuck.
async fn cron(args: Vec<String>) -> anyhow::Result<i32> {
    let mode = args.first().map(String::as_str).unwrap_or("weighted");
    let invocation_id = option_value(&args, "--invocation-id").unwrap_or_else(default_cron_id);
    match mode {
        "ok" => cron_once(CronOutcome::Ok, &invocation_id, 0).await,
        "fail" => cron_once(CronOutcome::Fail, &invocation_id, 0).await,
        "stuck" => cron_once(CronOutcome::Stuck, &invocation_id, 0).await,
        "duplicate" => {
            let first = cron_once(CronOutcome::Ok, &invocation_id, 1).await?;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let second = cron_once(CronOutcome::Ok, &invocation_id, 2).await?;
            Ok(first.max(second))
        }
        "missed" => Ok(0),
        "weighted" | "" => cron_once(weighted_cron_outcome(), &invocation_id, 0).await,
        other => Err(anyhow::anyhow!("unknown cron mode: {other}")),
    }
}

async fn cron_once(
    outcome: CronOutcome,
    invocation_id: &str,
    duplicate_ordinal: i64,
) -> anyhow::Result<i32> {
    let span = tracing::info_span!(
        "cron_job",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        "cron.job.name" = "playground-report",
        "cron.schedule" = "*/1 * * * *",
        "cron.invocation.id" = %invocation_id,
        "cron.outcome" = outcome.as_str(),
        "cron.duplicate.ordinal" = duplicate_ordinal
    );
    async move {
        match outcome {
            CronOutcome::Ok => {
                tracing::info!("cron job succeeded");
                Ok(0)
            }
            CronOutcome::Fail => {
                playground_telemetry::mark_span_error("nonzero_exit");
                tracing::error!("cron job failed");
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

fn default_session_id() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("exec-stack-{seconds}-{}", std::process::id())
}

fn default_cron_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("playground-report-{millis}-{}", std::process::id())
}

fn run_id(session: &str) -> String {
    std::env::var("PARALLAX_RUN_ID").unwrap_or_else(|_| session.to_string())
}

fn resource_attrs_with_run_id(run_id: &str) -> String {
    resource_attrs_with_run_id_from(
        std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok().as_deref(),
        run_id,
    )
}

fn resource_attrs_with_run_id_from(existing: Option<&str>, run_id: &str) -> String {
    let existing = existing.unwrap_or_default();
    if existing
        .split(',')
        .filter_map(|item| item.split_once('='))
        .any(|(key, _)| key.trim() == semconv::PARALLAX_RUN_ID)
    {
        return existing.to_owned();
    }
    if existing.trim().is_empty() {
        format!("{}={run_id}", semconv::PARALLAX_RUN_ID)
    } else {
        format!("{existing},{}={run_id}", semconv::PARALLAX_RUN_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::{CronOutcome, cron_once, resource_attrs_with_run_id_from};

    #[tokio::test]
    async fn cron_outcomes_preserve_process_exit_contract() {
        assert_eq!(
            cron_once(CronOutcome::Ok, "test-ok", 0)
                .await
                .expect("ok cron"),
            0
        );
        assert_eq!(
            cron_once(CronOutcome::Fail, "test-fail", 0)
                .await
                .expect("fail cron"),
            1
        );
    }

    #[test]
    fn resource_attributes_add_run_id_once() {
        assert_eq!(
            resource_attrs_with_run_id_from(None, "run-a"),
            "parallax.run.id=run-a"
        );
        assert_eq!(
            resource_attrs_with_run_id_from(Some("service.name=cli"), "run-a"),
            "service.name=cli,parallax.run.id=run-a"
        );
        assert_eq!(
            resource_attrs_with_run_id_from(Some("parallax.run.id=existing"), "run-a"),
            "parallax.run.id=existing"
        );
    }
}

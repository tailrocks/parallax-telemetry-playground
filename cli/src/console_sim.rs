//! Scripted interactive TUI session (plan 158 decision 8): the only current
//! emitter of `interactive` app mode — `session.start` → screen visits →
//! `ui.action` root spans (the checkout action crosses into the real
//! microservices) → `session.end`.

use tracing::Instrument;

use playground_telemetry::invocation;
use playground_telemetry::semconv;

/// One scripted screen visit: (screen, dwell budget share, actions).
const SCRIPT: &[(&str, &[&str])] = &[
    (semconv::APP_SCREEN_HOME, &[]),
    (semconv::APP_SCREEN_CART, &[semconv::UI_ACTION_CART_ADD]),
    (
        semconv::APP_SCREEN_CHECKOUT,
        &[
            semconv::UI_ACTION_CHECKOUT_SUBMIT,
            semconv::UI_ACTION_SCREEN_BACK,
        ],
    ),
];

pub(crate) struct ConsoleOptions {
    pub seconds: u64,
    /// Force this `ui.action.name` to fail with a checkout-screen error
    /// attributed to the submitting widget (`j-error`).
    pub fail_at: Option<String>,
    /// Emit an error log between screen visits so it lands in the
    /// journey's "outside any screen" bucket (`j-outside`).
    pub outside_error: bool,
    /// Run N chained sessions linked via `session.previous_id` (`j-reattach`).
    pub sessions: u32,
}

impl ConsoleOptions {
    fn parse(args: &[String]) -> anyhow::Result<Self> {
        Ok(Self {
            seconds: super::option_value(args, "--seconds")
                .map(|value| value.parse())
                .transpose()?
                .unwrap_or(30),
            fail_at: super::option_value(args, "--fail-at"),
            outside_error: super::flag_present(args, "--outside-error"),
            sessions: super::option_value(args, "--reattach")
                .map(|value| value.parse())
                .transpose()?
                .unwrap_or(1),
        })
    }
}

pub(crate) async fn run(args: Vec<String>) -> anyhow::Result<i32> {
    let options = ConsoleOptions::parse(&args)?;
    let invocation_id = invocation::invocation_id();
    let mut failures = 0;
    let mut previous: Option<String> = None;
    for _ in 0..options.sessions.max(1) {
        let session_id = invocation::new_session_id();
        failures += run_session(&options, &session_id, previous.as_deref()).await?;
        previous = Some(session_id);
    }
    tracing::info!(%invocation_id, failures, sessions = options.sessions, "console complete");
    Ok(if failures == 0 { 0 } else { 1 })
}

async fn run_session(
    options: &ConsoleOptions,
    session_id: &str,
    previous: Option<&str>,
) -> anyhow::Result<u32> {
    session_event(semconv::SESSION_START_EVENT_NAME, session_id, previous);
    let dwell = std::time::Duration::from_millis(
        (options.seconds * 1_000 / (SCRIPT.len() as u64 * u64::from(options.sessions.max(1))))
            .clamp(50, 10_000),
    );
    let mut failures = 0;
    for (sequence, (screen, actions)) in SCRIPT.iter().enumerate() {
        let visit_id = uuid::Uuid::new_v4().to_string();
        screen_event(
            semconv::UI_SCREEN_ENTERED_EVENT_NAME,
            session_id,
            screen,
            &visit_id,
            sequence as i64 + 1,
        );
        tokio::time::sleep(dwell / 2).await;
        for action in *actions {
            let forced_failure = options.fail_at.as_deref() == Some(*action);
            if !ui_action(action, screen, session_id, forced_failure).await {
                failures += 1;
            }
        }
        tokio::time::sleep(dwell / 2).await;
        screen_event(
            semconv::UI_SCREEN_EXITED_EVENT_NAME,
            session_id,
            screen,
            &visit_id,
            sequence as i64 + 1,
        );
        if options.outside_error && sequence == 0 {
            // Between visits: no screen is active, so the journey must file
            // this in the unattributed bucket, never drop it.
            outside_screen_error(session_id);
        }
    }
    session_event(semconv::SESSION_END_EVENT_NAME, session_id, None);
    Ok(failures)
}

fn outside_screen_error(session_id: &str) {
    let invocation_id = invocation::invocation_id();
    tracing::error!(
        cli.invocation.id = %invocation_id,
        session.id = %session_id,
        error.type = "console::BetweenScreens",
        "background refresh failed between screens"
    );
}

fn session_event(event: &'static str, session_id: &str, previous: Option<&str>) {
    let invocation_id = invocation::invocation_id();
    match previous {
        Some(previous) => tracing::info!(
            "event.name" = event,
            cli.invocation.id = %invocation_id,
            session.id = %session_id,
            session.previous_id = %previous,
            "console session event"
        ),
        None => tracing::info!(
            "event.name" = event,
            cli.invocation.id = %invocation_id,
            session.id = %session_id,
            "console session event"
        ),
    }
}

fn screen_event(
    event: &'static str,
    session_id: &str,
    screen: &str,
    visit_id: &str,
    sequence: i64,
) {
    let invocation_id = invocation::invocation_id();
    tracing::info!(
        "event.name" = event,
        cli.invocation.id = %invocation_id,
        session.id = %session_id,
        app.screen.id = %screen,
        ui.screen.visit.id = %visit_id,
        ui.navigation.sequence = sequence,
        ui.transition.reason = "user_navigation",
        "console screen event"
    );
}

/// One bounded user action as a root span (fresh trace); the checkout submit
/// calls the real checkout service so the trace crosses process boundaries.
async fn ui_action(
    action: &'static str,
    screen: &str,
    session_id: &str,
    forced_failure: bool,
) -> bool {
    let invocation_id = invocation::invocation_id();
    let span = tracing::info_span!(
        parent: None,
        "ui.action",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        cli.invocation.id = %invocation_id,
        session.id = %session_id,
        ui.action.name = %action,
        app.screen.id = %screen,
        app.widget.name = %format!("{action}.button"),
        outcome = tracing::field::Empty,
    );
    let handle = span.clone();
    async move {
        let ok = if forced_failure {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            false
        } else if action == semconv::UI_ACTION_CHECKOUT_SUBMIT {
            checkout_submit().await
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            true
        };
        if ok {
            handle.record("outcome", semconv::OUTCOME_SUCCESS);
            tracing::info!(%action, "console action succeeded");
        } else {
            handle.record("outcome", semconv::OUTCOME_ERROR);
            playground_telemetry::mark_span_error("action_failure");
            tracing::error!(
                %action,
                error.type = "console::ActionFailed",
                app.screen.id = %screen,
                app.widget.name = %format!("{action}.button"),
                session.id = %session_id,
                cli.invocation.id = %invocation::invocation_id(),
                "console action failed"
            );
        }
        ok
    }
    .instrument(span)
    .await
}

async fn checkout_submit() -> bool {
    let base = std::env::var("CHECKOUT_URL").unwrap_or_else(|_| "http://localhost:8088".into());
    let url = format!("{base}/checkout?sku=WIDGET-1&quantity=1");
    match playground_telemetry::traced_get(&url).await {
        Ok(response) => response.status().is_success(),
        Err(error) => {
            tracing::error!(%error, "checkout call failed");
            false
        }
    }
}

/// The deterministic emission order of one session, for structural tests:
/// (kind, screen, detail). Emission follows this order synchronously, so a
/// forced action failure is always inside its screen's enter/exit window.
#[cfg(test)]
fn session_script(options: &ConsoleOptions) -> Vec<(&'static str, &'static str, String)> {
    let mut steps = Vec::new();
    steps.push(("session.start", "", String::new()));
    for (sequence, (screen, actions)) in SCRIPT.iter().enumerate() {
        steps.push(("screen.entered", screen, String::new()));
        for action in *actions {
            let failed = options.fail_at.as_deref() == Some(*action);
            steps.push((
                if failed { "action.failed" } else { "action" },
                screen,
                format!("{action}|{action}.button"),
            ));
        }
        steps.push(("screen.exited", screen, String::new()));
        if options.outside_error && sequence == 0 {
            steps.push(("error.outside", "", String::new()));
        }
    }
    steps.push(("session.end", "", String::new()));
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(fail_at: Option<&str>, outside: bool) -> ConsoleOptions {
        ConsoleOptions {
            seconds: 1,
            fail_at: fail_at.map(str::to_string),
            outside_error: outside,
            sessions: 1,
        }
    }

    #[test]
    fn happy_script_pairs_every_screen_and_ends_the_session() {
        let steps = session_script(&options(None, false));
        assert_eq!(steps.first().unwrap().0, "session.start");
        assert_eq!(steps.last().unwrap().0, "session.end");
        let enters = steps.iter().filter(|s| s.0 == "screen.entered").count();
        let exits = steps.iter().filter(|s| s.0 == "screen.exited").count();
        assert_eq!(enters, SCRIPT.len());
        assert_eq!(exits, SCRIPT.len());
        assert!(steps.iter().filter(|s| s.0 == "action").count() >= 2);
    }

    #[test]
    fn forced_checkout_failure_lands_inside_the_checkout_visit_with_widget() {
        let steps = session_script(&options(Some(semconv::UI_ACTION_CHECKOUT_SUBMIT), false));
        let entered = steps
            .iter()
            .position(|s| s.0 == "screen.entered" && s.1 == semconv::APP_SCREEN_CHECKOUT)
            .expect("checkout entered");
        let exited = steps
            .iter()
            .position(|s| s.0 == "screen.exited" && s.1 == semconv::APP_SCREEN_CHECKOUT)
            .expect("checkout exited");
        let failure = steps
            .iter()
            .position(|s| s.0 == "action.failed")
            .expect("forced failure emitted");
        assert!(
            entered < failure && failure < exited,
            "failure must fall inside the checkout visit window"
        );
        assert_eq!(
            steps[failure].2,
            format!(
                "{}|{}.button",
                semconv::UI_ACTION_CHECKOUT_SUBMIT,
                semconv::UI_ACTION_CHECKOUT_SUBMIT
            ),
            "failure carries the action and widget context"
        );
        assert_eq!(steps[failure].1, semconv::APP_SCREEN_CHECKOUT);
    }

    #[test]
    fn outside_error_lands_between_screen_visits() {
        let steps = session_script(&options(None, true));
        let outside = steps
            .iter()
            .position(|s| s.0 == "error.outside")
            .expect("outside error emitted");
        assert_eq!(steps[outside - 1].0, "screen.exited");
        assert_eq!(steps[outside + 1].0, "screen.entered");
    }
}

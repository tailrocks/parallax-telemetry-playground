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

pub(crate) async fn run(args: Vec<String>) -> anyhow::Result<i32> {
    let seconds: u64 = super::option_value(&args, "--seconds")
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(30);
    let session_id = invocation::new_session_id();
    let invocation_id = invocation::invocation_id();

    session_event(semconv::SESSION_START_EVENT_NAME, &session_id, None);
    let dwell =
        std::time::Duration::from_millis((seconds * 1_000 / SCRIPT.len() as u64).clamp(50, 10_000));
    let mut failures = 0;
    for (sequence, (screen, actions)) in SCRIPT.iter().enumerate() {
        let visit_id = uuid::Uuid::new_v4().to_string();
        screen_event(
            semconv::UI_SCREEN_ENTERED_EVENT_NAME,
            &session_id,
            screen,
            &visit_id,
            sequence as i64 + 1,
        );
        tokio::time::sleep(dwell / 2).await;
        for action in *actions {
            if !ui_action(action, screen, &session_id).await {
                failures += 1;
            }
        }
        tokio::time::sleep(dwell / 2).await;
        screen_event(
            semconv::UI_SCREEN_EXITED_EVENT_NAME,
            &session_id,
            screen,
            &visit_id,
            sequence as i64 + 1,
        );
    }
    session_event(semconv::SESSION_END_EVENT_NAME, &session_id, None);
    tracing::info!(%invocation_id, %session_id, failures, "console session complete");
    Ok(if failures == 0 { 0 } else { 1 })
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
async fn ui_action(action: &'static str, screen: &str, session_id: &str) -> bool {
    let invocation_id = invocation::invocation_id();
    let span = tracing::info_span!(
        parent: None,
        "ui.action",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        cli.invocation.id = %invocation_id,
        session.id = %session_id,
        ui.action.name = %action,
        app.screen.id = %screen,
        outcome = tracing::field::Empty,
    );
    let handle = span.clone();
    async move {
        let ok = if action == semconv::UI_ACTION_CHECKOUT_SUBMIT {
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
            tracing::error!(%action, "console action failed");
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

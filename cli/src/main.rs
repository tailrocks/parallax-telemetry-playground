//! Playground driver CLI — short-lived process producing run-scoped telemetry.
//!   playground            drive the checkout flow (A1/A12)
//!   playground cron       a scheduled job with weighted outcomes (B17):
//!                         ~90% success, ~5% failure (nonzero exit),
//!                         ~5% "stuck" (long sleep → missed check-in)
//! Flushes telemetry on exit (short-lived discipline).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("playground-cli")?;
    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = match mode.as_str() {
        "cron" => cron().await,
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

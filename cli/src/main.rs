//! Playground driver CLI — short-lived process that drives the checkout flow,
//! producing a run-scoped trace. Stamps the run as a root span and flushes on
//! exit (short-lived telemetry discipline).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("playground-cli")?;
    let result = drive().await;
    telemetry.shutdown(); // flush before exit
    result
}

#[tracing::instrument(fields(otel.kind = "client"))]
async fn drive() -> anyhow::Result<()> {
    let base = std::env::var("CHECKOUT_URL").unwrap_or_else(|_| "http://localhost:8088".into());
    let url = format!("{base}/checkout?sku=WIDGET-1&quantity=3");
    let body = reqwest::get(&url).await?.text().await?;
    tracing::info!(%url, "drove checkout");
    println!("{body}");
    Ok(())
}

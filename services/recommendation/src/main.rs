#![allow(clippy::result_large_err)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("recommendation")?;
    let state = config::AppState::from_env()?;
    let app = api::app_with_state(state);
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8090".into());
    tracing::info!(%addr, "recommendation HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}
mod api;
mod catalog;
mod chaos;
mod config;
mod error;

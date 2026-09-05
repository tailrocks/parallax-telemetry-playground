#![allow(clippy::result_large_err)]

mod api;
mod application;
mod domain;
mod infrastructure;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("checkout")?;
    let address = std::env::var("CHECKOUT_ADDR").unwrap_or_else(|_| "0.0.0.0:8088".to_owned());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let (state, workers) = infrastructure::init_state(shutdown_receiver).await?;
    tracing::info!(%address, "checkout HTTP listening");
    let server_shutdown_sender = shutdown_sender.clone();
    let server = axum::serve(listener, api::app(state)).with_graceful_shutdown(async move {
        playground_telemetry::shutdown_signal().await;
        let _ = server_shutdown_sender.send(true);
    });
    let server_result = server.await;
    let _ = shutdown_sender.send(true);
    let worker_result = workers.shutdown().await;
    telemetry.shutdown();
    server_result?;
    worker_result?;
    Ok(())
}

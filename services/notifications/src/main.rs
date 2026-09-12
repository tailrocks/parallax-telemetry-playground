mod api;
mod domain;
mod store;

use anyhow::Context as _;
use std::time::Duration;
use tokio::task::JoinHandle;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("notifications")?;
    let state = store::AppState::new(store::init_db().await?)?;
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8091".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let app = api::app_with_state(state.clone());
    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let mut worker = tokio::spawn(store::run_delivery_worker(state, shutdown_receiver));
    tracing::info!(%addr, "notifications HTTP listening");
    let server_shutdown_sender = shutdown_sender.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        playground_telemetry::shutdown_signal().await;
        let _ = server_shutdown_sender.send(true);
    });
    let server_result = server.await;
    let _ = shutdown_sender.send(true);
    let worker_result = drain_worker(&mut worker).await;
    telemetry.shutdown();
    server_result?;
    worker_result?;
    Ok(())
}

const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

async fn drain_worker(worker: &mut JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    match tokio::time::timeout(WORKER_DRAIN_TIMEOUT, &mut *worker).await {
        Ok(result) => result
            .context("notification worker task join")?
            .context("notification worker stopped"),
        Err(_) => {
            worker.abort();
            let _ = (&mut *worker).await;
            Err(anyhow::anyhow!(
                "notification worker did not stop before the drain deadline"
            ))
        }
    }
}

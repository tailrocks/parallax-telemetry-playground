use inventory::{api, infrastructure};
use std::time::Duration;
use tokio::task::JoinHandle;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("inventory")?;
    let pool = infrastructure::init_db().await?;
    let address = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8089".to_owned());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let mut reclaimer =
        infrastructure::spawn_expired_reservation_reclaimer(pool.clone(), shutdown_receiver);
    tracing::info!(%address, "inventory listening");
    let server_shutdown_sender = shutdown_sender.clone();
    let server = axum::serve(listener, api::router(pool)).with_graceful_shutdown(async move {
        playground_telemetry::shutdown_signal().await;
        let _ = server_shutdown_sender.send(true);
    });
    let server_result = server.await;
    let _ = shutdown_sender.send(true);
    let reclaimer_result = drain_reclaimer(&mut reclaimer).await;
    telemetry.shutdown();
    server_result?;
    reclaimer_result?;
    Ok(())
}

const RECLAIMER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

async fn drain_reclaimer(reclaimer: &mut JoinHandle<()>) -> anyhow::Result<()> {
    match tokio::time::timeout(RECLAIMER_DRAIN_TIMEOUT, &mut *reclaimer).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!(
            "inventory reservation reclaimer task failed: {error}"
        )),
        Err(_) => {
            reclaimer.abort();
            let _ = (&mut *reclaimer).await;
            Err(anyhow::anyhow!(
                "inventory reservation reclaimer did not stop before the drain deadline"
            ))
        }
    }
}

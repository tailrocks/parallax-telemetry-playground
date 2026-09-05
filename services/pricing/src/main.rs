use playground_proto::pricing::v1::pricing_server::PricingServer;
use pricing::{api::PricingSvc, infrastructure::init_state};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tonic_health::server::health_reporter;

const READINESS_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("pricing")?;
    let state = Arc::new(init_state().await?);
    let address = std::env::var("PRICING_ADDR").unwrap_or_else(|_| "0.0.0.0:50051".to_owned());
    tracing::info!(
        %address,
        redis = state.redis_available(),
        "pricing gRPC listening"
    );
    let (health_reporter, health_service) = health_reporter();
    let initial_ready = state.database_ready().await;
    if initial_ready {
        health_reporter
            .set_serving::<PricingServer<PricingSvc>>()
            .await;
    } else {
        health_reporter
            .set_not_serving::<PricingServer<PricingSvc>>()
            .await;
    }
    let address = address.parse()?;
    let readiness_state = Arc::clone(&state);
    let pricing = PricingSvc::new(state);
    let shutdown_sender = pricing.shutdown_sender();
    let readiness_reporter = health_reporter.clone();
    let readiness_shutdown_sender = shutdown_sender.clone();
    let readiness_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        let mut was_ready = initial_ready;
        let mut shutdown = readiness_shutdown_sender.subscribe();
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.changed() => return,
        }
        loop {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown.changed() => return,
            }
            let ready = tokio::select! {
                ready = readiness_state.database_ready() => ready,
                _ = shutdown.changed() => return,
            };
            if ready != was_ready {
                if ready {
                    tracing::info!("pricing database became ready");
                } else {
                    tracing::warn!("pricing database became unavailable");
                }
                was_ready = ready;
            }
            if ready {
                tokio::select! {
                    _ = readiness_reporter.set_serving::<PricingServer<PricingSvc>>() => {}
                    _ = shutdown.changed() => return,
                }
            } else {
                tokio::select! {
                    _ = readiness_reporter.set_not_serving::<PricingServer<PricingSvc>>() => {}
                    _ = shutdown.changed() => return,
                }
            }
        }
    });
    let server_shutdown_sender = shutdown_sender.clone();
    let server = Server::builder()
        .add_service(health_service)
        .add_service(PricingServer::new(pricing.clone()))
        .serve_with_shutdown(address, async move {
            playground_telemetry::shutdown_signal().await;
            let _ = server_shutdown_sender.send(true);
        });
    let server_result = server.await;
    let _ = shutdown_sender.send(true);
    let readiness_result = drain_readiness_task(readiness_handle).await;
    let stream_result = pricing.shutdown().await;
    telemetry.shutdown();
    server_result?;
    readiness_result?;
    stream_result?;
    Ok(())
}

async fn drain_readiness_task(mut task: JoinHandle<()>) -> anyhow::Result<()> {
    match tokio::time::timeout(READINESS_DRAIN_TIMEOUT, &mut task).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!(
            "pricing readiness refresher task failed: {error}"
        )),
        Err(_) => {
            tracing::error!(
                "pricing readiness refresher did not stop before the drain deadline; aborting"
            );
            task.abort();
            let _ = task.await;
            Err(anyhow::anyhow!(
                "pricing readiness refresher did not stop before the drain deadline"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn readiness_task_is_cancelled_and_joined() {
        let (shutdown, mut receiver) = tokio::sync::watch::channel(false);
        let completed = Arc::new(AtomicBool::new(false));
        let task_completed = completed.clone();
        let task = tokio::spawn(async move {
            let _ = receiver.changed().await;
            task_completed.store(true, Ordering::Release);
        });

        assert!(shutdown.send(true).is_ok());
        drain_readiness_task(task)
            .await
            .expect("readiness task drains");

        assert!(completed.load(Ordering::Acquire));
    }
}

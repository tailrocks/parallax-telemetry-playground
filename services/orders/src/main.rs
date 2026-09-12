mod api;
mod messaging;

use anyhow::Context as _;
use std::time::Duration;
use tokio::task::JoinHandle;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("orders")?;
    let state = messaging::broker()
        .await
        .context("connect required RabbitMQ broker")?;
    let address = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8092".to_owned());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let consumer_state = state.clone();
    let mut consumer =
        tokio::spawn(
            async move { messaging::consume_loop(consumer_state, shutdown_receiver).await },
        );
    let server_shutdown_sender = shutdown_sender.clone();
    let server_shutdown_receiver = shutdown_sender.subscribe();
    let server = axum::serve(listener, api::app(state))
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = playground_telemetry::shutdown_signal() => {
                    let _ = server_shutdown_sender.send(true);
                }
                _ = wait_for_shutdown(server_shutdown_receiver) => {}
            }
        })
        .into_future();
    tokio::pin!(server);
    let (server_result, consumer_result) = tokio::select! {
        result = &mut server => {
            let _ = shutdown_sender.send(true);
            let consumer_result = drain_consumer(&mut consumer).await;
            (result.context("orders HTTP server"), consumer_result)
        }
        result = &mut consumer => {
            let _ = shutdown_sender.send(true);
            let consumer_result = result
                .context("orders consumer task join")
                .and_then(|result| result)
                .map_err(|error| error.context("orders consumer stopped"));
            (server.await.context("orders HTTP server"), consumer_result)
        }
    };
    let _ = shutdown_sender.send(true);
    telemetry.shutdown();
    server_result?;
    consumer_result
}

const CONSUMER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

async fn wait_for_shutdown(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let _ = shutdown.changed().await;
}

async fn drain_consumer(consumer: &mut JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    match tokio::time::timeout(CONSUMER_DRAIN_TIMEOUT, &mut *consumer).await {
        Ok(result) => result
            .context("orders consumer task join")?
            .context("orders consumer stopped"),
        Err(_) => {
            consumer.abort();
            let _ = (&mut *consumer).await;
            Err(anyhow::anyhow!(
                "orders consumer did not stop before the drain deadline"
            ))
        }
    }
}

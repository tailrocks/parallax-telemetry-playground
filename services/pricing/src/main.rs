//! Pricing gRPC service (tonic). Quotes a price for a SKU; instrumented so each
//! Quote is a SERVER span stitched into the caller's trace.
// tonic's `Status` is intentionally large; the gRPC trait signatures return it
// by value, so this lint is unavoidable for generated service impls.
#![allow(clippy::result_large_err)]
use playground_proto::pricing::v1::pricing_server::{Pricing, PricingServer};
use playground_proto::pricing::v1::{QuoteRequest, QuoteResponse};
use playground_telemetry::semconv;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, transport::Server};
use tracing::Instrument;

#[derive(Default)]
struct PricingSvc;

type QuoteStreamS = Pin<Box<dyn tokio_stream::Stream<Item = Result<QuoteResponse, Status>> + Send>>;

#[tonic::async_trait]
impl Pricing for PricingSvc {
    async fn quote(
        &self,
        request: Request<QuoteRequest>,
    ) -> Result<Response<QuoteResponse>, Status> {
        let span = tracing::info_span!("quote", otel.kind = semconv::SPAN_KIND_SERVER);
        let parent = playground_telemetry::extract_grpc_context(request.metadata());
        playground_telemetry::set_parent_from_grpc_metadata(&span, request.metadata());
        playground_telemetry::stamp_business_baggage(&span, &parent);
        async move {
            let grpc_timeout = grpc_timeout(request.metadata());
            let req = request.into_inner();
            if req.delay_ms > 0 {
                if let Some(timeout) =
                    grpc_timeout.filter(|timeout| Duration::from_millis(u64::from(req.delay_ms)) > *timeout)
                {
                    let guard = Duration::from_millis(10);
                    let wait = timeout.saturating_sub(guard);
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                    playground_telemetry::mark_span_error("deadline_exceeded");
                    tracing::warn!(
                        delay_ms = req.delay_ms,
                        timeout_ms = timeout.as_millis() as u64,
                        "pricing quote exceeded grpc-timeout"
                    );
                    return Err(Status::deadline_exceeded("deadline exceeded"));
                }
                tokio::time::sleep(Duration::from_millis(u64::from(req.delay_ms))).await;
            }
            // Deterministic toy pricing: 1999 minor units per unit.
            let total_minor = 1999u64 * u64::from(req.quantity.max(1));
            tracing::info!(sku = %req.sku, quantity = req.quantity, delay_ms = req.delay_ms, total_minor, "quoted");
            Ok(Response::new(QuoteResponse {
                sku: req.sku,
                quantity: req.quantity,
                total_minor,
                currency: "USD".into(),
            }))
        }
        .instrument(span)
        .await
    }

    type QuoteStreamStream = QuoteStreamS;

    /// A7: server-streaming — one QuoteResponse per unit (a long-lived stream span).
    async fn quote_stream(
        &self,
        request: Request<QuoteRequest>,
    ) -> Result<Response<Self::QuoteStreamStream>, Status> {
        let span = tracing::info_span!("quote_stream", otel.kind = semconv::SPAN_KIND_SERVER);
        let parent = playground_telemetry::extract_grpc_context(request.metadata());
        playground_telemetry::set_parent_from_grpc_metadata(&span, request.metadata());
        playground_telemetry::stamp_business_baggage(&span, &parent);
        async move {
            let req = request.into_inner();
            let n = req.quantity.max(1);
            let sku = req.sku;
            let delay_ms = if req.delay_ms > 0 { req.delay_ms } else { 50 };
            let fail_at = req.fail_at;
            tracing::info!(%sku, n, delay_ms, fail_at, "streaming quotes");
            let (tx, rx) = mpsc::channel::<Result<QuoteResponse, Status>>(16);
            let stream_span = tracing::Span::current();
            tokio::spawn(
                async move {
                    for i in 1..=n {
                        tokio::time::sleep(Duration::from_millis(u64::from(delay_ms))).await;
                        if fail_at > 0 && i == fail_at {
                            playground_telemetry::mark_span_error("stream_failed");
                            tracing::error!(
                                "rpc.message.type" = "SENT",
                                "rpc.message.id" = i64::from(i),
                                "pricing stream failed at requested item"
                            );
                            let _ = tx
                                .send(Err(Status::internal("pricing stream failed")))
                                .await;
                            return;
                        }
                        tracing::info!(
                            "rpc.message.type" = "SENT",
                            "rpc.message.id" = i64::from(i),
                            "rpc.message"
                        );
                        if tx
                            .send(Ok(QuoteResponse {
                                sku: sku.clone(),
                                quantity: i,
                                total_minor: 1999u64 * u64::from(i),
                                currency: "USD".into(),
                            }))
                            .await
                            .is_err()
                        {
                            playground_telemetry::mark_span_error("stream_cancelled");
                            tracing::warn!(
                                sent = i.saturating_sub(1),
                                "pricing stream cancelled by client"
                            );
                            return;
                        }
                    }
                }
                .instrument(stream_span),
            );
            let stream: QuoteStreamS = Box::pin(ReceiverStream::new(rx));
            Ok(Response::new(stream))
        }
        .instrument(span)
        .await
    }
}

fn grpc_timeout(metadata: &MetadataMap) -> Option<Duration> {
    let value = metadata.get("grpc-timeout")?.to_str().ok()?;
    let (amount, unit) = value.split_at(value.len().checked_sub(1)?);
    let amount: u64 = amount.parse().ok()?;
    match unit {
        "H" => Some(Duration::from_secs(amount.saturating_mul(60 * 60))),
        "M" => Some(Duration::from_secs(amount.saturating_mul(60))),
        "S" => Some(Duration::from_secs(amount)),
        "m" => Some(Duration::from_millis(amount)),
        "u" => Some(Duration::from_micros(amount)),
        "n" => Some(Duration::from_nanos(amount)),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("pricing")?;
    let addr = std::env::var("PRICING_ADDR").unwrap_or_else(|_| "0.0.0.0:50051".into());
    tracing::info!(%addr, "pricing gRPC listening");
    Server::builder()
        .add_service(PricingServer::new(PricingSvc))
        .serve_with_shutdown(addr.parse()?, playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use playground_proto::pricing::v1::pricing_client::PricingClient;
    use tokio_stream::wrappers::TcpListenerStream;

    async fn client() -> (
        PricingClient<tonic::transport::Channel>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            Server::builder()
                .add_service(PricingServer::new(PricingSvc))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("server exits cleanly");
        });
        let client = PricingClient::connect(format!("http://{address}"))
            .await
            .expect("client connects");
        (client, shutdown_tx)
    }

    #[test]
    fn parses_grpc_timeout_units() {
        let mut metadata = MetadataMap::new();
        metadata.insert("grpc-timeout", "100000u".parse().unwrap());
        assert_eq!(grpc_timeout(&metadata), Some(Duration::from_millis(100)));

        metadata.insert("grpc-timeout", "2S".parse().unwrap());
        assert_eq!(grpc_timeout(&metadata), Some(Duration::from_secs(2)));
    }

    #[tokio::test]
    async fn serves_unary_and_streaming_quotes_over_grpc() {
        let (mut client, shutdown) = client().await;
        let response = client
            .quote(QuoteRequest {
                sku: "WIDGET-1".into(),
                quantity: 3,
                delay_ms: 0,
                fail_at: 0,
            })
            .await
            .expect("unary quote succeeds")
            .into_inner();
        assert_eq!(response.total_minor, 5997);

        let mut stream = client
            .quote_stream(QuoteRequest {
                sku: "WIDGET-1".into(),
                quantity: 3,
                delay_ms: 1,
                fail_at: 0,
            })
            .await
            .expect("stream starts")
            .into_inner();
        let mut quantities = Vec::new();
        while let Some(response) = stream.message().await.expect("stream message") {
            quantities.push(response.quantity);
        }
        assert_eq!(quantities, vec![1, 2, 3]);
        shutdown.send(()).expect("shutdown signal sends");
    }

    #[tokio::test]
    async fn surfaces_requested_stream_failure_over_grpc() {
        let (mut client, shutdown) = client().await;
        let mut stream = client
            .quote_stream(QuoteRequest {
                sku: "WIDGET-1".into(),
                quantity: 3,
                delay_ms: 1,
                fail_at: 2,
            })
            .await
            .expect("stream starts")
            .into_inner();
        assert_eq!(
            stream
                .message()
                .await
                .expect("first item")
                .expect("response")
                .quantity,
            1
        );
        let status = stream
            .message()
            .await
            .expect_err("requested failure surfaces");
        assert_eq!(status.code(), tonic::Code::Internal);
        shutdown.send(()).expect("shutdown signal sends");
    }
}

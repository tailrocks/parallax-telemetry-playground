//! Pricing gRPC service (tonic). Quotes a price for a SKU; instrumented so each
//! Quote is a SERVER span stitched into the caller's trace.
use playground_proto::pricing::v1::pricing_server::{Pricing, PricingServer};
use playground_proto::pricing::v1::{QuoteRequest, QuoteResponse};
use tonic::{Request, Response, Status, transport::Server};

#[derive(Default)]
struct PricingSvc;

#[tonic::async_trait]
impl Pricing for PricingSvc {
    #[tracing::instrument(skip(self, request), fields(otel.kind = "server"))]
    async fn quote(
        &self,
        request: Request<QuoteRequest>,
    ) -> Result<Response<QuoteResponse>, Status> {
        let req = request.into_inner();
        // Deterministic toy pricing: 1999 minor units per unit.
        let total_minor = 1999u64 * u64::from(req.quantity.max(1));
        tracing::info!(sku = %req.sku, quantity = req.quantity, total_minor, "quoted");
        Ok(Response::new(QuoteResponse {
            sku: req.sku,
            quantity: req.quantity,
            total_minor,
            currency: "USD".into(),
        }))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("pricing")?;
    let addr = std::env::var("PRICING_ADDR").unwrap_or_else(|_| "0.0.0.0:50051".into());
    tracing::info!(%addr, "pricing gRPC listening");
    Server::builder()
        .add_service(PricingServer::new(PricingSvc))
        .serve(addr.parse()?)
        .await?;
    telemetry.shutdown();
    Ok(())
}

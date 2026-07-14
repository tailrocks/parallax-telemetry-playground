//! notifications HTTP service (scaffold) — telemetry-wired axum server. Flesh out the
//! domain behavior per docs (DB spans / cache / reverse-hop) at implementation.
use axum::http::HeaderMap;
use axum::{Router, routing::get};
use playground_telemetry::semconv;
use tracing::Instrument;

async fn handle(headers: HeaderMap) -> &'static str {
    let span = tracing::info_span!("handle", otel.kind = semconv::SPAN_KIND_SERVER);
    playground_telemetry::set_parent_from_headers(&span, &headers);
    handle_inner().instrument(span).await
}

async fn handle_inner() -> &'static str {
    tracing::info!("notifications handled request");
    "notifications ok"
}

fn app() -> Router {
    Router::new()
        .route("/", get(handle))
        .route("/healthz", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("notifications")?;
    let app = app();
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8091".into());
    tracing::info!(%addr, "notifications HTTP listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::app;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_notification_and_health_boundaries() {
        let response = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), b"notifications ok");

        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

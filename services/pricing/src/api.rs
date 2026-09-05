use crate::{
    application::{apply_scenario_delay, calculate_quote},
    domain::{request_id, to_proto},
    infrastructure::AppState,
};
use opentelemetry::context::FutureExt as _;
use playground_proto::pricing::v1::{QuoteRequest, QuoteResponse, pricing_server::Pricing};
use playground_telemetry::semconv;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, mpsc, watch},
    task::JoinSet,
};
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::Instrument;

const STREAM_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct PricingSvc {
    state: Arc<AppState>,
    stream_tasks: Arc<StreamTasks>,
}

impl PricingSvc {
    pub fn new(state: Arc<AppState>) -> Self {
        let (shutdown, receiver) = watch::channel(false);
        Self {
            state,
            stream_tasks: Arc::new(StreamTasks {
                shutdown,
                receiver,
                tasks: Mutex::new(JoinSet::new()),
            }),
        }
    }

    pub fn shutdown_sender(&self) -> watch::Sender<bool> {
        self.stream_tasks.shutdown.clone()
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.stream_tasks.shutdown().await
    }
}

struct StreamTasks {
    shutdown: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
    tasks: Mutex<JoinSet<()>>,
}

impl StreamTasks {
    fn subscribe(&self) -> watch::Receiver<bool> {
        self.receiver.clone()
    }

    async fn spawn<F>(&self, task: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::error!(error = %error, "pricing stream task failed");
            }
        }
        if *self.receiver.borrow() {
            return false;
        }
        tasks.spawn(task);
        true
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        let _ = self.shutdown.send(true);
        let mut tasks = {
            let mut registered = self.tasks.lock().await;
            std::mem::replace(&mut *registered, JoinSet::new())
        };
        drain_stream_tasks(&mut tasks).await
    }
}

async fn drain_stream_tasks(tasks: &mut JoinSet<()>) -> anyhow::Result<()> {
    let deadline = Instant::now() + STREAM_DRAIN_TIMEOUT;
    let mut first_error = None;
    while !tasks.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = match tokio::time::timeout(remaining, tasks.join_next()).await {
            Ok(Some(result)) => result,
            Ok(None) => break,
            Err(_) => {
                tracing::error!(
                    "pricing stream task did not stop before the drain deadline; aborting"
                );
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return Err(anyhow::anyhow!(
                    "pricing stream task did not stop before the drain deadline"
                ));
            }
        };
        if let Err(error) = result {
            tracing::error!(error = %error, "pricing stream task failed during shutdown");
            if first_error.is_none() {
                first_error = Some(anyhow::anyhow!(
                    "pricing stream task failed during shutdown: {error}"
                ));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

type QuoteStreamResult = Pin<Box<dyn Stream<Item = Result<QuoteResponse, Status>> + Send>>;

fn with_inbound_context<F>(
    future: F,
    span: tracing::Span,
    parent: opentelemetry::Context,
) -> impl Future<Output = F::Output>
where
    F: Future,
{
    future.instrument(span).with_context(parent)
}

#[tonic::async_trait]
impl Pricing for PricingSvc {
    async fn quote(
        &self,
        request: Request<QuoteRequest>,
    ) -> Result<Response<QuoteResponse>, Status> {
        let tenant_id = playground_telemetry::resolve_grpc_tenant_identity(
            request.metadata(),
            Some(&request.get_ref().tenant_id),
        )
        .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let span = tracing::info_span!(
            "pricing.quote",
            otel.kind = semconv::SPAN_KIND_SERVER,
            "cache.result" = tracing::field::Empty,
            cache.system = "redis"
        );
        let parent = playground_telemetry::extract_grpc_context(request.metadata());
        playground_telemetry::set_parent_from_grpc_metadata(&span, request.metadata());
        playground_telemetry::stamp_business_baggage(&span, &parent);
        with_inbound_context(
            async move {
                let mut request = request.into_inner();
                request.tenant_id = tenant_id;
                apply_scenario_delay(&request.context, &request_id(&request)).await?;
                let quote = calculate_quote(&self.state, &request).await?;
                tracing::info!(quote_id = %quote.quote_id, line_count = quote.lines.len(), total_minor = quote.grand_total_minor, "pricing quote calculated");
                Ok(Response::new(to_proto(&quote)))
            },
            span,
            parent,
        )
        .await
    }

    type QuoteStreamStream = QuoteStreamResult;

    async fn quote_stream(
        &self,
        request: Request<QuoteRequest>,
    ) -> Result<Response<Self::QuoteStreamStream>, Status> {
        let tenant_id = playground_telemetry::resolve_grpc_tenant_identity(
            request.metadata(),
            Some(&request.get_ref().tenant_id),
        )
        .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let span = tracing::info_span!(
            "pricing.quote_stream",
            otel.kind = semconv::SPAN_KIND_SERVER
        );
        // The extractor applies the shared bounded business-baggage allowlist
        // before this context crosses into the detached task.
        let parent = playground_telemetry::extract_grpc_context(request.metadata());
        playground_telemetry::set_parent_from_grpc_metadata(&span, request.metadata());
        playground_telemetry::stamp_business_baggage(&span, &parent);
        let mut request = request.into_inner();
        request.tenant_id = tenant_id;
        crate::domain::validate_request(&request)?;
        let state = self.state.clone();
        let mut shutdown = self.stream_tasks.subscribe();
        let (sender, receiver) = mpsc::channel(16);
        let stream_parent = parent.clone();
        let stream_task = with_inbound_context(
            async move {
                let calculation_span = tracing::info_span!(
                    "pricing.quote_stream.calculate",
                    otel.kind = semconv::SPAN_KIND_INTERNAL,
                    "cache.result" = tracing::field::Empty,
                    cache.system = "redis"
                );
                playground_telemetry::stamp_business_baggage(&calculation_span, &stream_parent);
                let base_request_id = request_id(&request);
                for end in 1..=request.items.len() {
                    if *shutdown.borrow() {
                        return;
                    }
                    let mut prefix_request = request.clone();
                    prefix_request.items.truncate(end);
                    prefix_request.request_id = format!("{base_request_id}:stream:{end}");
                    let result = tokio::select! {
                        result = calculate_quote(&state, &prefix_request).instrument(calculation_span.clone()) => result,
                        _ = sender.closed() => {
                            tracing::info!(sent = end.saturating_sub(1), "pricing stream cancelled during quote calculation");
                            return;
                        }
                        _ = shutdown.changed() => {
                            tracing::info!(sent = end.saturating_sub(1), "pricing stream cancelled during quote calculation");
                            return;
                        }
                    };
                    match result {
                        Ok(quote) => {
                            if *shutdown.borrow() {
                                return;
                            }
                            let send_span = tracing::info_span!(
                                "pricing.quote_stream.send",
                                otel.kind = semconv::SPAN_KIND_INTERNAL,
                                "rpc.message.type" = "SENT",
                                "rpc.message.id" = end as i64,
                            );
                            playground_telemetry::stamp_business_baggage(
                                &send_span,
                                &stream_parent,
                            );
                            let send_result = tokio::select! {
                                result = async {
                                    let result = sender.send(Ok(to_proto(&quote))).await;
                                    if result.is_ok() {
                                        tracing::info!(
                                            "rpc.message.type" = "SENT",
                                            "rpc.message.id" = end as i64,
                                            "pricing quote snapshot sent"
                                        );
                                    }
                                    result
                                }.instrument(send_span) => result,
                                _ = shutdown.changed() => {
                                    tracing::info!(sent = end.saturating_sub(1), "pricing stream cancelled");
                                    return;
                                }
                            };
                            if send_result.is_err() {
                                tracing::warn!(
                                    sent = end.saturating_sub(1),
                                    "pricing stream cancelled"
                                );
                                return;
                            }
                        }
                        Err(error) => {
                            let send_span = tracing::info_span!(
                                "pricing.quote_stream.send",
                                otel.kind = semconv::SPAN_KIND_INTERNAL,
                                "rpc.message.type" = "ERROR",
                            );
                            playground_telemetry::stamp_business_baggage(
                                &send_span,
                                &stream_parent,
                            );
                            let _ = tokio::select! {
                                result = sender.send(Err(error)).instrument(send_span) => result,
                                _ = shutdown.changed() => {
                                    tracing::info!(sent = end.saturating_sub(1), "pricing stream cancelled");
                                    return;
                                }
                            };
                            return;
                        }
                    }
                }
            },
            span.clone(),
            parent,
        );
        if !self.stream_tasks.spawn(stream_task).await {
            return Err(Status::unavailable("pricing service is shutting down"));
        }
        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::baggage::BaggageExt;
    use opentelemetry::{Context, KeyValue};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn stream_tasks_cancel_and_join_before_shutdown_returns() {
        let (shutdown, receiver) = watch::channel(false);
        let stream_tasks = StreamTasks {
            shutdown,
            receiver,
            tasks: Mutex::new(JoinSet::new()),
        };
        let completed = Arc::new(AtomicBool::new(false));
        let task_completed = completed.clone();
        let mut receiver = stream_tasks.subscribe();
        assert!(
            stream_tasks
                .spawn(async move {
                    let _ = receiver.changed().await;
                    task_completed.store(true, Ordering::Release);
                })
                .await
        );

        stream_tasks.shutdown().await.expect("stream tasks drain");

        assert!(completed.load(Ordering::Acquire));
        assert!(!stream_tasks.spawn(async {}).await);
    }

    #[tokio::test]
    async fn inbound_context_survives_unary_downstream_await() {
        let parent = Context::new().with_baggage([
            KeyValue::new("tenant.id", "tenant-acme"),
            KeyValue::new("customer.segment", "returning"),
        ]);
        let before = Context::current()
            .baggage()
            .get("customer.segment")
            .map(ToString::to_string);
        let observed = with_inbound_context(
            async {
                tokio::task::yield_now().await;
                Context::current()
                    .baggage()
                    .get("customer.segment")
                    .map(ToString::to_string)
            },
            tracing::info_span!("pricing.test.unary_downstream"),
            parent,
        )
        .await;

        assert_eq!(observed.as_deref(), Some("returning"));
        assert_eq!(
            Context::current()
                .baggage()
                .get("customer.segment")
                .map(ToString::to_string),
            before
        );
    }

    #[tokio::test]
    async fn inbound_context_survives_detached_stream_work() {
        let parent = Context::new().with_baggage([KeyValue::new("tenant.id", "tenant-acme")]);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = with_inbound_context(
            async move {
                tokio::task::yield_now().await;
                let observed = Context::current()
                    .baggage()
                    .get("tenant.id")
                    .map(ToString::to_string);
                let _ = sender.send(observed);
            },
            tracing::info_span!("pricing.test.stream_downstream"),
            parent,
        );

        tokio::spawn(task).await.expect("stream task joins");
        assert_eq!(
            receiver.await.expect("stream task reports"),
            Some("tenant-acme".to_owned())
        );
    }
}

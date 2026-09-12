package dev.tailrocks.fulfillment;

import com.rabbitmq.client.Channel;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.Scope;
import io.tailrocks.semconv.Semconv;
import java.io.IOException;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.rabbit.annotation.RabbitListener;
import org.springframework.stereotype.Component;

@Component
final class AnalyticsEventConsumer {
    private final com.fasterxml.jackson.databind.ObjectMapper mapper;
    private final FulfillmentRepository repository;
    private final ClickHouseAnalyticsClient clickhouse;
    private final OrderEventPublisher publisher;
    private final String analyticsQueueName;

    AnalyticsEventConsumer(
        com.fasterxml.jackson.databind.ObjectMapper mapper,
        FulfillmentRepository repository,
        ClickHouseAnalyticsClient clickhouse,
        OrderEventPublisher publisher,
        @Value("${fulfillment.queues.analytics:analytics.events}") String analyticsQueueName
    ) {
        this.mapper = mapper;
        this.repository = repository;
        this.clickhouse = clickhouse;
        this.publisher = publisher;
        this.analyticsQueueName = analyticsQueueName;
    }

    @RabbitListener(
        queues = "${fulfillment.queues.analytics:analytics.events}",
        containerFactory = "rabbitListenerContainerFactory"
    )
    void onEvent(Message message, Channel channel) throws IOException {
        Context extracted;
        try {
            RabbitMessagingConfiguration.requireBrokerIdentity(message);
            extracted = RabbitTraceContext.extract(message.getMessageProperties());
        } catch (PermanentEventException error) {
            channel.basicReject(message.getMessageProperties().getDeliveryTag(), false);
            return;
        }
        Span consumer = RabbitTraceContext.startConsumerSpan(
            analyticsQueueName,
            extracted,
            message.getMessageProperties()
        );
        Context consumerContext = consumer.getSpanContext().isValid()
            ? extracted.with(consumer)
            : extracted;
        CommerceEvent event = null;
        AnalyticsClaim claim = null;
        try (Scope ignored = consumerContext.makeCurrent()) {
            event = CommerceEvent.parse(
                mapper,
                message.getBody(),
                message.getMessageProperties().getHeaders(),
                message.getMessageProperties().getMessageId()
            );
            RabbitDeliveryRoute.from(message.getMessageProperties())
                .validateForAnalytics(event, analyticsQueueName);
            consumer.setAttribute(Semconv.MESSAGING_SYSTEM, "rabbitmq");
            consumer.setAttribute(
                Semconv.MESSAGING_DESTINATION_NAME,
                analyticsQueueName
            );
            consumer.setAttribute(Semconv.MESSAGING_OPERATION_NAME, "process");
            consumer.setAttribute(Semconv.MESSAGING_MESSAGE_ID, event.eventId());
            consumer.setAttribute(Semconv.TENANT_ID, event.tenantId());
            consumer.setAttribute("commerce.event.type", event.eventType());

            claim = repository.claimAnalytics(event);
            if (claim.leaseActive()) {
                deferUntilClaimLeaseExpires(message, channel, Context.current());
                return;
            }
            if (claim.deadLettered()) {
                channel.basicReject(message.getMessageProperties().getDeliveryTag(), false);
                return;
            }
            if (claim.duplicate()) {
                channel.basicAck(message.getMessageProperties().getDeliveryTag(), false);
                return;
            }
            try (LeaseHeartbeat heartbeat = repository.maintainLease(
                event.tenantId(),
                FulfillmentRepository.ANALYTICS_CONSUMER,
                event.eventKey(),
                claim.leaseToken()
            )) {
                heartbeat.verify();
                repository.renewLease(
                    event.tenantId(),
                    FulfillmentRepository.ANALYTICS_CONSUMER,
                    event.eventKey(),
                    claim.leaseToken()
                );
                heartbeat.verify();
                clickhouse.ingest(event);
                heartbeat.verify();
                repository.markCompleted(
                    event.tenantId(),
                    FulfillmentRepository.ANALYTICS_CONSUMER,
                    event.eventKey(),
                    claim.leaseToken()
                );
                consumer.setAttribute(Semconv.OUTCOME, Semconv.OUTCOME_SUCCESS);
                channel.basicAck(message.getMessageProperties().getDeliveryTag(), false);
            }
        } catch (PermanentEventException error) {
            if (event != null && claim != null && claim.state() == ClaimState.ACQUIRED) {
                try {
                    repository.markFailed(
                        FulfillmentRepository.ANALYTICS_CONSUMER,
                        event.tenantId(),
                        event.eventKey(),
                        claim.leaseToken(),
                        error
                    );
                } catch (RuntimeException markFailedError) {
                    error.addSuppressed(markFailedError);
                    throw markFailedError;
                }
            }
            channel.basicReject(message.getMessageProperties().getDeliveryTag(), false);
            consumer.setAttribute(Semconv.OUTCOME, Semconv.OUTCOME_FAILURE);
        } catch (RuntimeException error) {
            if (event != null && claim != null && claim.state() == ClaimState.ACQUIRED) {
                try {
                    repository.markFailed(
                        FulfillmentRepository.ANALYTICS_CONSUMER,
                        event.tenantId(),
                        event.eventKey(),
                        claim.leaseToken(),
                        error
                    );
                } catch (RuntimeException markFailedError) {
                    error.addSuppressed(markFailedError);
                }
            }
            consumer.setAttribute(Semconv.OUTCOME, Semconv.OUTCOME_FAILURE);
            throw error;
        } finally {
            consumer.end();
        }
    }

    private void deferUntilClaimLeaseExpires(
        Message message,
        Channel channel,
        Context context
    ) throws IOException {
        long deliveryTag = message.getMessageProperties().getDeliveryTag();
        try {
            publisher.defer(
                message,
                RabbitMessagingConfiguration.analyticsRetryRoutingKey(analyticsQueueName),
                context
            );
            channel.basicAck(deliveryTag, false);
        } catch (PermanentEventException error) {
            channel.basicReject(deliveryTag, false);
        } catch (RuntimeException | IOException error) {
            try {
                channel.basicNack(deliveryTag, false, true);
            } catch (IOException nackError) {
                error.addSuppressed(nackError);
                throw new RetryableEventException(
                    "failed to defer active analytics claim", error);
            }
        }
    }
}

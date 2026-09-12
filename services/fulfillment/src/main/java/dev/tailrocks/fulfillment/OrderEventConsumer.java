package dev.tailrocks.fulfillment;

import com.rabbitmq.client.Channel;
import io.opentelemetry.api.common.AttributeKey;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.Scope;
import io.tailrocks.semconv.Semconv;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.List;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.rabbit.annotation.RabbitListener;
import org.springframework.stereotype.Component;

@Component
final class OrderEventConsumer {
    private final com.fasterxml.jackson.databind.ObjectMapper mapper;
    private final FulfillmentRepository repository;
    private final OrderEventPublisher publisher;
    private final NotificationClient notifications;

    OrderEventConsumer(
        com.fasterxml.jackson.databind.ObjectMapper mapper,
        FulfillmentRepository repository,
        OrderEventPublisher publisher,
        NotificationClient notifications
    ) {
        this.mapper = mapper;
        this.repository = repository;
        this.publisher = publisher;
        this.notifications = notifications;
    }

    @RabbitListener(
        queues = "${fulfillment.queues.orders:fulfillment.orders}",
        containerFactory = "rabbitListenerContainerFactory"
    )
    void onOrder(Message message, Channel channel) throws IOException {
        Context extracted;
        try {
            RabbitMessagingConfiguration.requireBrokerIdentity(message);
            extracted = RabbitTraceContext.extract(message.getMessageProperties());
        } catch (PermanentEventException error) {
            channel.basicReject(message.getMessageProperties().getDeliveryTag(), false);
            return;
        }
        Span consumer = RabbitTraceContext.startConsumerSpan(
            RabbitMessagingConfiguration.ORDERS_QUEUE,
            extracted,
            message.getMessageProperties()
        );
        Context consumerContext = consumer.getSpanContext().isValid()
            ? extracted.with(consumer)
            : extracted;
        CommerceEvent event = null;
        ProcessingResult result = null;
        FulfillmentEffect activeEffect = null;
        try (Scope ignored = consumerContext.makeCurrent()) {
            event = CommerceEvent.parse(
                mapper,
                message.getBody(),
                message.getMessageProperties().getHeaders(),
                message.getMessageProperties().getMessageId()
            );
            RabbitDeliveryRoute.from(message.getMessageProperties()).validate(event);
            consumer.setAttribute(
                AttributeKey.stringKey(Semconv.OTEL_KIND),
                Semconv.SPAN_KIND_CONSUMER
            );
            consumer.setAttribute(Semconv.MESSAGING_SYSTEM, "rabbitmq");
            consumer.setAttribute(
                Semconv.MESSAGING_DESTINATION_NAME,
                RabbitMessagingConfiguration.ORDERS_QUEUE
            );
            consumer.setAttribute(Semconv.MESSAGING_OPERATION_NAME, "process");
            consumer.setAttribute(Semconv.MESSAGING_MESSAGE_ID, event.eventId());
            consumer.setAttribute(Semconv.TENANT_ID, event.tenantId());
            consumer.setAttribute("order.id", event.orderId());
            consumer.setAttribute("commerce.event.type", event.eventType());

            result = repository.processOrder(event);
            if (result.leaseActive()) {
                deferUntilClaimLeaseExpires(message, channel, Context.current());
                return;
            }
            if (result.deadLettered()) {
                channel.basicReject(message.getMessageProperties().getDeliveryTag(), false);
                return;
            }
            if (result.duplicate()) {
                channel.basicAck(message.getMessageProperties().getDeliveryTag(), false);
                return;
            }

            try (LeaseHeartbeat heartbeat = repository.maintainLease(
                event.tenantId(),
                FulfillmentRepository.ORDER_CONSUMER,
                event.processingKey(),
                result.leaseToken()
            )) {
                heartbeat.verify();
                Context downstreamContext = Context.current();
                repository.renewLease(
                    event.tenantId(),
                    FulfillmentRepository.ORDER_CONSUMER,
                    event.processingKey(),
                    result.leaseToken()
                );
                heartbeat.verify();
                List<FulfillmentEffect> effects = repository.claimEffects(
                    event,
                    result.leaseToken()
                );
                for (FulfillmentEffect effect : effects) {
                    activeEffect = effect;
                    heartbeat.verify();
                    repository.validateEffectClaim(
                        effect,
                        event.processingKey(),
                        result.leaseToken()
                    );
                    heartbeat.verify();
                    dispatch(effect, downstreamContext);
                    heartbeat.verify();
                    repository.publishEffect(
                        effect,
                        event.processingKey(),
                        result.leaseToken()
                    );
                    activeEffect = null;
                }
                repository.markCompleted(
                    event.tenantId(),
                    FulfillmentRepository.ORDER_CONSUMER,
                    event.processingKey(),
                    result.leaseToken()
                );
                consumer.setAttribute(Semconv.OUTCOME, Semconv.OUTCOME_SUCCESS);
                channel.basicAck(message.getMessageProperties().getDeliveryTag(), false);
            }
        } catch (PermanentEventException error) {
            if (activeEffect != null) {
                try {
                    repository.releaseEffect(activeEffect, error);
                } catch (RuntimeException releaseError) {
                    error.addSuppressed(releaseError);
                }
            }
            markPermanentClaimFailed(event, result, error);
            channel.basicReject(message.getMessageProperties().getDeliveryTag(), false);
            consumer.setAttribute(Semconv.OUTCOME, Semconv.OUTCOME_FAILURE);
        } catch (RuntimeException error) {
            if (activeEffect != null) {
                try {
                    repository.releaseEffect(activeEffect, error);
                } catch (RuntimeException releaseError) {
                    error.addSuppressed(releaseError);
                }
            }
            if (event != null && result != null && result.acquired()) {
                try {
                    repository.markFailed(
                        FulfillmentRepository.ORDER_CONSUMER,
                        event.tenantId(),
                        event.processingKey(),
                        result.leaseToken(),
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

    private void markPermanentClaimFailed(
        CommerceEvent event,
        ProcessingResult result,
        PermanentEventException error
    ) {
        if (event == null || result == null || !result.acquired()) {
            return;
        }
        try {
            repository.markFailed(
                FulfillmentRepository.ORDER_CONSUMER,
                event.tenantId(),
                event.processingKey(),
                result.leaseToken(),
                error
            );
        } catch (RuntimeException markFailedError) {
            error.addSuppressed(markFailedError);
            throw markFailedError;
        }
    }

    private void dispatch(FulfillmentEffect effect, Context context) {
        CommerceEvent event = CommerceEvent.parse(
            mapper,
            effect.payload().getBytes(StandardCharsets.UTF_8)
        );
        if (effect.effectKind().equals("event")) {
            publisher.publish(event, context);
            return;
        }
        if (effect.effectKind().equals("notification")
            && effect.notificationStatus() != null
            && !effect.notificationStatus().isBlank()) {
            notifications.notifyOrder(event, effect.notificationStatus(), context);
            return;
        }
        throw new PermanentEventException("unknown fulfillment effect kind");
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
                RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY,
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
                    "failed to defer active fulfillment claim", error);
            }
        }
    }
}

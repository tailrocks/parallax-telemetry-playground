package dev.tailrocks.fulfillment;

import io.opentelemetry.api.trace.Span;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.Scope;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.core.MessageDeliveryMode;
import org.springframework.amqp.core.MessageProperties;
import org.springframework.amqp.core.MessagePropertiesBuilder;
import org.springframework.amqp.rabbit.connection.CorrelationData;
import org.springframework.amqp.rabbit.core.RabbitTemplate;
import org.springframework.stereotype.Component;

@Component
final class OrderEventPublisher {
    private static final long CONFIRM_TIMEOUT_SECONDS = 10;

    private final RabbitTemplate rabbit;
    private final com.fasterxml.jackson.databind.ObjectMapper mapper;

    OrderEventPublisher(
        RabbitTemplate rabbit,
        com.fasterxml.jackson.databind.ObjectMapper mapper
    ) {
        this.rabbit = rabbit;
        this.mapper = mapper;
    }

    void publish(CommerceEvent event, Context context) {
        String messageId = event.eventId();
        Span producer = RabbitTraceContext.startPublishSpan(
            RabbitMessagingConfiguration.EVENTS_EXCHANGE,
            messageId,
            event.eventType(),
            context
        );
        Context producerContext = producer.getSpanContext().isValid()
            ? context.with(producer)
            : context;
        try (Scope ignored = producerContext.makeCurrent()) {
            MessageProperties properties = new MessageProperties();
            properties.setContentType("application/json");
            properties.setContentEncoding(StandardCharsets.UTF_8.name());
            properties.setDeliveryMode(MessageDeliveryMode.PERSISTENT);
            properties.setMessageId(messageId);
            properties.setHeader("event-key", event.eventKey());
            properties.setHeader("event-type", event.eventType());
            properties.setHeader("tenant-id", event.tenantId());
            RabbitTraceContext.injectRequired(producerContext, properties);

            CorrelationData correlation = new CorrelationData(event.eventKey());
            send(
                RabbitMessagingConfiguration.EVENTS_EXCHANGE,
                event.eventType(),
                new Message(event.toJson(mapper).getBytes(StandardCharsets.UTF_8), properties),
                correlation
            );
            awaitConfirm(event.eventKey(), correlation);
        } finally {
            producer.end();
        }
    }

    void defer(Message source, String routingKey, Context context) {
        MessageProperties properties = MessagePropertiesBuilder
            .fromClonedProperties(source.getMessageProperties())
            .build();
        properties.setDeliveryMode(MessageDeliveryMode.PERSISTENT);
        properties.setDeliveryTag(0);
        RabbitDeliveryRoute.preserveOriginalRoute(source.getMessageProperties(), properties);
        RabbitTraceContext.injectRequired(context, properties);
        String correlationId = properties.getMessageId();
        if (correlationId == null || correlationId.isBlank()) {
            correlationId = routingKey;
        }
        CorrelationData correlation = new CorrelationData(correlationId + ":" + routingKey);
        send(
            RabbitMessagingConfiguration.RETRY_EXCHANGE,
            routingKey,
            new Message(source.getBody(), properties),
            correlation
        );
        awaitConfirm(correlation.getId(), correlation);
    }

    private void send(String exchange, String routingKey, Message message, CorrelationData correlation) {
        rabbit.send(exchange, routingKey, message, correlation);
    }

    private void awaitConfirm(String eventKey, CorrelationData correlation) {
        try {
            var confirmation = correlation.getFuture()
                .get(CONFIRM_TIMEOUT_SECONDS, TimeUnit.SECONDS);
            if (confirmation == null || !confirmation.ack()) {
                String cause = confirmation == null ? "no confirmation" : confirmation.reason();
                throw new RetryableEventException(
                    "RabbitMQ publisher confirm rejected event " + eventKey + ": " + cause);
            }
            var returned = correlation.getReturned();
            if (returned != null) {
                throw new RetryableEventException(
                    "RabbitMQ returned event " + eventKey
                        + " code=" + returned.getReplyCode()
                        + " reason=" + returned.getReplyText()
                        + " exchange=" + returned.getExchange()
                        + " routingKey=" + returned.getRoutingKey()
                );
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new RetryableEventException("interrupted waiting for RabbitMQ confirm", error);
        } catch (TimeoutException error) {
            throw new RetryableEventException("timed out waiting for RabbitMQ confirm", error);
        } catch (java.util.concurrent.ExecutionException error) {
            throw new RetryableEventException("RabbitMQ publisher confirm failed", error.getCause());
        }
    }
}

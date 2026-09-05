package dev.tailrocks.fulfillment;

import java.nio.charset.StandardCharsets;
import jakarta.annotation.PostConstruct;
import org.springframework.amqp.AmqpRejectAndDontRequeueException;
import org.springframework.amqp.core.AcknowledgeMode;
import org.springframework.amqp.core.Binding;
import org.springframework.amqp.core.BindingBuilder;
import org.springframework.amqp.core.ExchangeBuilder;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.core.MessageProperties;
import org.springframework.amqp.core.Queue;
import org.springframework.amqp.core.QueueBuilder;
import org.springframework.amqp.core.TopicExchange;
import org.springframework.amqp.rabbit.annotation.EnableRabbit;
import org.springframework.amqp.rabbit.config.SimpleRabbitListenerContainerFactory;
import org.springframework.amqp.rabbit.config.StatelessRetryOperationsInterceptor;
import org.springframework.amqp.rabbit.connection.ConnectionFactory;
import org.springframework.amqp.rabbit.core.RabbitTemplate;
import org.springframework.amqp.rabbit.retry.MessageRecoverer;
import org.springframework.amqp.listener.ListenerExecutionFailedException;
import org.springframework.amqp.rabbit.config.RetryInterceptorBuilder;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;
import org.springframework.beans.factory.annotation.Qualifier;
import org.springframework.beans.factory.annotation.Value;

@Configuration
@EnableRabbit
class RabbitMessagingConfiguration {
    static final String EVENTS_EXCHANGE = "commerce.events";
    static final String DEAD_LETTER_EXCHANGE = "commerce.dlx";
    static final String ORDERS_QUEUE = "fulfillment.orders";
    static final String ORDERS_DEAD_LETTER_QUEUE = "fulfillment.orders.dead";
    static final String ANALYTICS_QUEUE = "analytics.events";
    static final String ORDERS_DEAD_LETTER_ROUTE = "fulfillment.orders.dead";
    static final String RETRY_EXCHANGE = "commerce.retry";
    static final String RETRY_RETURN_EXCHANGE = "commerce.retry.return";
    /**
     * Listener consumers read this after the configured queues have been declared.
     * Keep the default for tests and direct consumers before Spring initialization.
     */
    static volatile String ORDERS_RETRY_ROUTING_KEY = ORDERS_QUEUE + ".retry";
    static final int BROKER_RETRY_COUNT = FulfillmentRepository.MAX_CLAIM_ATTEMPTS - 1;

    @Value("${fulfillment.queues.orders:fulfillment.orders}")
    private String ordersQueueName = ORDERS_QUEUE;

    @Value("${fulfillment.queues.analytics:analytics.events}")
    private String analyticsQueueName = ANALYTICS_QUEUE;

    @Value("${fulfillment.claim-lease-seconds:30}")
    private long claimLeaseSeconds = 30;

    @Value("${spring.rabbitmq.listener.simple.prefetch:20}")
    private int listenerPrefetch = 20;

    @Value("${spring.rabbitmq.listener.simple.concurrency:1}")
    private int listenerConcurrency = 1;

    @Value("${spring.rabbitmq.listener.simple.max-concurrency:4}")
    private int listenerMaxConcurrency = 4;

    @PostConstruct
    void configureOrderRouting() {
        configuredOrdersRetryRoutingKey();
    }

    @Bean
    TopicExchange commerceEventsExchange() {
        return ExchangeBuilder.topicExchange(EVENTS_EXCHANGE).durable(true).build();
    }

    @Bean
    TopicExchange commerceDeadLetterExchange() {
        return ExchangeBuilder.topicExchange(DEAD_LETTER_EXCHANGE).durable(true).build();
    }

    @Bean
    TopicExchange commerceRetryExchange() {
        return ExchangeBuilder.topicExchange(RETRY_EXCHANGE).durable(true).build();
    }

    @Bean
    TopicExchange commerceRetryReturnExchange() {
        return ExchangeBuilder.topicExchange(RETRY_RETURN_EXCHANGE).durable(true).build();
    }

    @Bean
    Queue fulfillmentOrdersQueue() {
        String queueName = configuredOrdersQueueName();
        configuredOrdersRetryRoutingKey();
        return QueueBuilder.durable(queueName)
            .withArgument("x-dead-letter-exchange", DEAD_LETTER_EXCHANGE)
            .withArgument("x-dead-letter-routing-key", queueName + ".dead")
            .build();
    }

    @Bean
    Queue fulfillmentOrdersDeadLetterQueue() {
        return QueueBuilder.durable(configuredOrdersQueueName() + ".dead").build();
    }

    @Bean
    Queue fulfillmentOrdersRetryQueue() {
        String retryRoutingKey = configuredOrdersRetryRoutingKey();
        return QueueBuilder.durable(retryRoutingKey)
            .withArgument("x-message-ttl", claimLeaseTtlMillis())
            .withArgument("x-dead-letter-exchange", RETRY_RETURN_EXCHANGE)
            .withArgument("x-dead-letter-routing-key", retryRoutingKey)
            .build();
    }

    @Bean
    Queue fulfillmentAnalyticsQueue() {
        String queueName = configuredAnalyticsQueueName();
        return QueueBuilder.durable(queueName)
            .withArgument("x-dead-letter-exchange", DEAD_LETTER_EXCHANGE)
            .withArgument("x-dead-letter-routing-key", analyticsDeadLetterRoutingKey(queueName))
            .build();
    }

    @Bean
    Queue fulfillmentAnalyticsDeadLetterQueue() {
        return QueueBuilder.durable(analyticsDeadLetterQueueName()).build();
    }

    @Bean
    Queue fulfillmentAnalyticsRetryQueue() {
        String queueName = configuredAnalyticsQueueName();
        return QueueBuilder.durable(queueName + ".retry")
            .withArgument("x-message-ttl", claimLeaseTtlMillis())
            .withArgument("x-dead-letter-exchange", RETRY_RETURN_EXCHANGE)
            .withArgument("x-dead-letter-routing-key", analyticsRetryRoutingKey(queueName))
            .build();
    }

    @Bean
    Binding orderEventsBinding(
        @Qualifier("fulfillmentOrdersQueue") Queue fulfillmentOrdersQueue,
        @Qualifier("commerceEventsExchange") TopicExchange commerceEventsExchange
    ) {
        return BindingBuilder.bind(fulfillmentOrdersQueue)
            .to(commerceEventsExchange)
            .with("order.#");
    }

    @Bean
    Binding paymentEventsBinding(
        @Qualifier("fulfillmentOrdersQueue") Queue fulfillmentOrdersQueue,
        @Qualifier("commerceEventsExchange") TopicExchange commerceEventsExchange
    ) {
        return BindingBuilder.bind(fulfillmentOrdersQueue)
            .to(commerceEventsExchange)
            .with("payment.#");
    }

    @Bean
    Binding orderRetryReturnBinding(
        @Qualifier("fulfillmentOrdersQueue") Queue fulfillmentOrdersQueue,
        @Qualifier("commerceRetryReturnExchange") TopicExchange commerceRetryReturnExchange
    ) {
        return BindingBuilder.bind(fulfillmentOrdersQueue)
            .to(commerceRetryReturnExchange)
            .with(configuredOrdersRetryRoutingKey());
    }

    @Bean
    Binding orderDeadLetterBinding(
        @Qualifier("fulfillmentOrdersDeadLetterQueue") Queue fulfillmentOrdersDeadLetterQueue,
        @Qualifier("commerceDeadLetterExchange") TopicExchange commerceDeadLetterExchange
    ) {
        return BindingBuilder.bind(fulfillmentOrdersDeadLetterQueue)
            .to(commerceDeadLetterExchange)
            .with(configuredOrdersQueueName() + ".dead");
    }

    @Bean
    Binding ordersRetryBinding(
        @Qualifier("fulfillmentOrdersRetryQueue") Queue fulfillmentOrdersRetryQueue,
        @Qualifier("commerceRetryExchange") TopicExchange commerceRetryExchange
    ) {
        return BindingBuilder.bind(fulfillmentOrdersRetryQueue)
            .to(commerceRetryExchange)
            .with(configuredOrdersRetryRoutingKey());
    }

    @Bean
    Binding analyticsEventsBinding(
        @Qualifier("fulfillmentAnalyticsQueue") Queue fulfillmentAnalyticsQueue,
        @Qualifier("commerceEventsExchange") TopicExchange commerceEventsExchange
    ) {
        return BindingBuilder.bind(fulfillmentAnalyticsQueue)
            .to(commerceEventsExchange)
            .with("#");
    }

    @Bean
    Binding analyticsRetryReturnBinding(
        @Qualifier("fulfillmentAnalyticsQueue") Queue fulfillmentAnalyticsQueue,
        @Qualifier("commerceRetryReturnExchange") TopicExchange commerceRetryReturnExchange
    ) {
        return BindingBuilder.bind(fulfillmentAnalyticsQueue)
            .to(commerceRetryReturnExchange)
            .with(analyticsRetryRoutingKey());
    }

    @Bean
    Binding analyticsDeadLetterBinding(
        @Qualifier("fulfillmentAnalyticsDeadLetterQueue") Queue fulfillmentAnalyticsDeadLetterQueue,
        @Qualifier("commerceDeadLetterExchange") TopicExchange commerceDeadLetterExchange
    ) {
        return BindingBuilder.bind(fulfillmentAnalyticsDeadLetterQueue)
            .to(commerceDeadLetterExchange)
            .with(analyticsDeadLetterRoutingKey());
    }

    @Bean
    Binding analyticsRetryBinding(
        @Qualifier("fulfillmentAnalyticsRetryQueue") Queue fulfillmentAnalyticsRetryQueue,
        @Qualifier("commerceRetryExchange") TopicExchange commerceRetryExchange
    ) {
        return BindingBuilder.bind(fulfillmentAnalyticsRetryQueue)
            .to(commerceRetryExchange)
            .with(analyticsRetryRoutingKey());
    }

    @Bean
    StatelessRetryOperationsInterceptor rabbitRetryInterceptor() {
        return RetryInterceptorBuilder.stateless()
            .maxRetries(BROKER_RETRY_COUNT)
            .backOffOptions(250, 2.0, 3_000)
            .recoverer(manualRejectingRecoverer())
            .build();
    }

    static MessageRecoverer manualRejectingRecoverer() {
        return (message, cause) -> {
            throw new ListenerExecutionFailedException(
                "Retry policy exhausted",
                new AmqpRejectAndDontRequeueException(
                    "Retry policy exhausted",
                    true,
                    cause
                ),
                message
            );
        };
    }

    @Bean(name = "rabbitListenerContainerFactory")
    SimpleRabbitListenerContainerFactory rabbitListenerContainerFactory(
        ConnectionFactory connectionFactory,
        StatelessRetryOperationsInterceptor retryInterceptor
    ) {
        validateListenerSettings();
        var factory = new SimpleRabbitListenerContainerFactory();
        factory.setConnectionFactory(connectionFactory);
        factory.setAcknowledgeMode(AcknowledgeMode.MANUAL);
        factory.setDefaultRequeueRejected(false);
        factory.setPrefetchCount(listenerPrefetch);
        factory.setConcurrentConsumers(listenerConcurrency);
        factory.setMaxConcurrentConsumers(listenerMaxConcurrency);
        factory.setAdviceChain(retryInterceptor);
        return factory;
    }

    @Bean
    RabbitTemplate rabbitTemplate(ConnectionFactory connectionFactory) {
        var template = new RabbitTemplate(connectionFactory);
        template.setMandatory(true);
        template.setConfirmCallback((correlation, acknowledged, cause) -> {
            if (!acknowledged) {
                String key = correlation == null ? "unknown" : correlation.getId();
                System.err.printf("RabbitMQ publisher confirm rejected event=%s cause=%s%n", key, cause);
            }
        });
        template.setReturnsCallback(returned -> {
            var properties = returned.getMessage().getMessageProperties();
            System.err.printf(
                "RabbitMQ publisher return messageId=%s code=%d reason=%s exchange=%s routingKey=%s%n",
                properties.getMessageId(),
                returned.getReplyCode(),
                returned.getReplyText(),
                returned.getExchange(),
                returned.getRoutingKey()
            );
        });
        return template;
    }

    private void validateListenerSettings() {
        if (listenerPrefetch < 1) {
            throw new IllegalStateException("spring.rabbitmq.listener.simple.prefetch must be positive");
        }
        if (listenerConcurrency < 1) {
            throw new IllegalStateException("spring.rabbitmq.listener.simple.concurrency must be positive");
        }
        if (listenerMaxConcurrency < listenerConcurrency) {
            throw new IllegalStateException(
                "spring.rabbitmq.listener.simple.max-concurrency must be >= concurrency"
            );
        }
    }

    private int claimLeaseTtlMillis() {
        if (claimLeaseSeconds <= 0 || claimLeaseSeconds > Integer.MAX_VALUE / 1_000L) {
            throw new IllegalStateException("fulfillment.claim-lease-seconds must fit a positive RabbitMQ TTL");
        }
        return Math.toIntExact(claimLeaseSeconds * 1_000L);
    }

    static String analyticsRetryRoutingKey(String queueName) {
        return requireAnalyticsQueueName(queueName) + ".retry";
    }

    static String ordersRetryRoutingKey(String queueName) {
        return requireQueueName(queueName) + ".retry";
    }

    static String analyticsDeadLetterRoutingKey(String queueName) {
        return requireAnalyticsQueueName(queueName) + ".dead";
    }

    private String configuredAnalyticsQueueName() {
        return requireAnalyticsQueueName(analyticsQueueName);
    }

    private String configuredOrdersQueueName() {
        return requireQueueName(ordersQueueName);
    }

    private String configuredOrdersRetryRoutingKey() {
        String routingKey = ordersRetryRoutingKey(configuredOrdersQueueName());
        ORDERS_RETRY_ROUTING_KEY = routingKey;
        return routingKey;
    }

    private String analyticsRetryRoutingKey() {
        return analyticsRetryRoutingKey(configuredAnalyticsQueueName());
    }

    private String analyticsDeadLetterRoutingKey() {
        return analyticsDeadLetterRoutingKey(configuredAnalyticsQueueName());
    }

    private String analyticsDeadLetterQueueName() {
        return analyticsDeadLetterRoutingKey();
    }

    private static String requireAnalyticsQueueName(String queueName) {
        return requireQueueName(queueName, "fulfillment.queues.analytics");
    }

    private static String requireQueueName(String queueName) {
        return requireQueueName(queueName, "fulfillment.queues.orders");
    }

    private static String requireQueueName(String queueName, String property) {
        if (queueName == null || queueName.isBlank()) {
            throw new IllegalStateException(property + " must not be blank");
        }
        return queueName.trim();
    }

    /**
     * Validate broker-owned identity at the listener boundary. This must run
     * inside the listener, where a permanent failure can be rejected to the
     * queue's dead-letter exchange. A throwing after-receive post processor is
     * outside that acknowledgement path and can redeliver one poison message
     * forever, starving every later delivery on the queue.
     */
    static Message requireBrokerIdentity(Message message) {
        if (message == null || message.getMessageProperties() == null) {
            throw new PermanentEventException("RabbitMQ message properties are required");
        }
        MessageProperties properties = message.getMessageProperties();
        requireHeader(properties, "event-key");
        requireHeader(properties, "event-type");
        requireHeader(properties, "tenant-id");
        if (properties.getMessageId() == null || properties.getMessageId().isBlank()) {
            throw new PermanentEventException("RabbitMQ message-id is required");
        }
        RabbitTraceContext.require(properties);
        return message;
    }

    private static void requireHeader(MessageProperties properties, String name) {
        Object value = properties.getHeader(name);
        String text = value instanceof byte[] bytes
            ? new String(bytes, StandardCharsets.UTF_8).trim()
            : value == null ? "" : value.toString().trim();
        if (text.isBlank()) {
            throw new PermanentEventException("RabbitMQ " + name + " header is required");
        }
    }
}

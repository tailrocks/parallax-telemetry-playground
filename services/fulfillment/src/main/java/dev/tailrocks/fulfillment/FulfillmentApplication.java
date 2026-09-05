package dev.tailrocks.fulfillment;

import com.fasterxml.jackson.databind.ObjectMapper;
import java.time.Duration;
import java.util.concurrent.ThreadFactory;
import org.springframework.context.annotation.Bean;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.client.SimpleClientHttpRequestFactory;
import org.springframework.web.client.RestClient;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;

@SpringBootApplication
public class FulfillmentApplication {
    public static void main(String[] args) {
        SpringApplication.run(FulfillmentApplication.class, args);
    }

    @Bean
    ObjectMapper objectMapper() {
        return new ObjectMapper().findAndRegisterModules();
    }

    @Bean
    RestClient.Builder restClientBuilder(
        @Value("${fulfillment.http.connect-timeout-ms:2000}") int connectTimeoutMillis,
        @Value("${fulfillment.http.read-timeout-ms:10000}") int readTimeoutMillis
    ) {
        validateTimeout("fulfillment.http.connect-timeout-ms", connectTimeoutMillis);
        validateTimeout("fulfillment.http.read-timeout-ms", readTimeoutMillis);
        var requestFactory = new SimpleClientHttpRequestFactory();
        requestFactory.setConnectTimeout(Duration.ofMillis(connectTimeoutMillis));
        requestFactory.setReadTimeout(Duration.ofMillis(readTimeoutMillis));
        return RestClient.builder().requestFactory(requestFactory);
    }

    @Bean(destroyMethod = "shutdown")
    ScheduledExecutorService fulfillmentLeaseExecutor(
        @Value("${spring.rabbitmq.listener.simple.max-concurrency:4}") int maxConcurrentConsumers,
        @Value("${fulfillment.claim-lease-heartbeat-threads:8}") int heartbeatThreads
    ) {
        if (maxConcurrentConsumers < 1) {
            throw new IllegalStateException(
                "spring.rabbitmq.listener.simple.max-concurrency must be positive"
            );
        }
        int requiredThreads;
        try {
            // One listener container exists for each durable queue. Reserve one
            // heartbeat worker per possible claim in both containers.
            requiredThreads = Math.multiplyExact(maxConcurrentConsumers, 2);
        } catch (ArithmeticException error) {
            throw new IllegalStateException("fulfillment listener concurrency is too large", error);
        }
        if (heartbeatThreads < requiredThreads) {
            throw new IllegalStateException(
                "fulfillment.claim-lease-heartbeat-threads must be >= twice listener max-concurrency"
            );
        }
        return Executors.newScheduledThreadPool(heartbeatThreads, fulfillmentThreadFactory());
    }

    private static ThreadFactory fulfillmentThreadFactory() {
        return runnable -> {
            Thread thread = new Thread(runnable, "fulfillment-lease-heartbeat");
            thread.setDaemon(true);
            return thread;
        };
    }

    private static void validateTimeout(String property, int timeoutMillis) {
        if (timeoutMillis < 1 || timeoutMillis > 60_000) {
            throw new IllegalStateException(property + " must be between 1 and 60000 milliseconds");
        }
    }
}

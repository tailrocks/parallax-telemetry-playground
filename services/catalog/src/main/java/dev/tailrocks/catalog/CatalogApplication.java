package dev.tailrocks.catalog;

import io.tailrocks.semconv.Semconv;

import dev.openfeature.contrib.providers.flagd.FlagdProvider;
import dev.openfeature.sdk.BooleanHook;
import dev.openfeature.sdk.FlagEvaluationDetails;
import dev.openfeature.sdk.HookContext;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.BatchMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.graphql.data.method.annotation.SchemaMapping;
import org.springframework.graphql.data.method.annotation.SubscriptionMapping;
import org.springframework.stereotype.Controller;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.ResponseBody;
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Client;
import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.logs.Severity;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.Tracer;
import io.opentelemetry.context.Scope;
import io.micrometer.core.instrument.Counter;
import io.micrometer.core.instrument.MeterRegistry;
import org.slf4j.LoggerFactory;
import org.slf4j.MDC;
import reactor.core.publisher.Flux;

import java.util.ArrayList;
import java.util.Collections;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.stream.Collectors;

@SpringBootApplication
public class CatalogApplication {
    public static void main(String[] args) {
        OpenFeatureAPI api = OpenFeatureAPI.getInstance();
        api.addHooks(new FeatureFlagSpanEventHook());
        api.setProvider(new FlagdProvider());
        SpringApplication.run(CatalogApplication.class, args);
    }
}

record Product(String id, String sku, String name, int priceMinor) {}

record Review(String text, int stars) {}

record HeapPressureResult(int requestedMb, int allocatedMb, long requestedHoldMs, long heldMs) {}

@Controller
class ProductController {
    private static final org.slf4j.Logger LOG = LoggerFactory.getLogger(ProductController.class);
    private static final io.opentelemetry.api.logs.Logger EVENT_LOGGER =
        GlobalOpenTelemetry.get().getLogsBridge().get("catalog.events");
    private static final int MAX_HEAP_MB = 256;
    private static final long MAX_HEAP_HOLD_MS = 30_000;
    private static final String PARTIAL_ERROR_SKU = "GADGET-1";
    private static final Tracer TRACER = GlobalOpenTelemetry.getTracer("catalog-graphql-scenarios");
    private static final List<Product> CATALOG = List.of(
        new Product("1", "WIDGET-1", "Widget", 1999),
        new Product("2", "GADGET-1", "Gadget", 4999)
    );

    @QueryMapping
    Product product(@Argument String sku) {
        return CATALOG.stream().filter(p -> p.sku().equals(sku)).findFirst().orElse(null);
    }

    // A14: OpenFeature flag evaluation (flagd provider) — the evaluation is
    // surfaced as feature_flag.* telemetry by the OTel hook at runtime.
    private final Client flags = OpenFeatureAPI.getInstance().getClient();
    // A2: a Micrometer counter — exported via OTLP with trace exemplars when
    // OTEL_METRICS_EXEMPLAR_FILTER=trace_based, so a metric data point links to
    // the trace that produced it. Exemplars are real on the JVM tier (the Rust
    // SDK has none yet).
    private final Counter productQueries;

    ProductController(MeterRegistry meters) {
        this.productQueries = Counter.builder(Semconv.CATALOG_PRODUCT_QUERIES)
            .description("product list queries")
            .register(meters);
    }

    @QueryMapping
    List<Product> products() {
        productQueries.increment();
        boolean promo = flags.getBooleanValue("catalogPromo", false);
        Span.current().setAttribute("catalog.promo", promo);
        var products = new ArrayList<>(CATALOG);
        if (promo) {
            Collections.reverse(products);
        }
        try (
            var ignoredEvent = MDC.putCloseable(Semconv.EVENT_NAME, Semconv.CATALOG_PRODUCTS_SERVED);
            var ignoredCount = MDC.putCloseable("product.count", String.valueOf(products.size()));
            var ignoredPromo = MDC.putCloseable("catalog.promo", String.valueOf(promo))
        ) {
            LOG.atInfo()
                .addKeyValue(Semconv.EVENT_NAME, Semconv.CATALOG_PRODUCTS_SERVED)
                .addKeyValue("product.count", products.size())
                .addKeyValue("catalog.promo", promo)
                .log("catalog products served");
        }
        emitCatalogProductsServed(products.size(), promo);
        return products;
    }

    private static void emitCatalogProductsServed(int productCount, boolean promo) {
        EVENT_LOGGER.logRecordBuilder()
            .setEventName(Semconv.CATALOG_PRODUCTS_SERVED)
            .setSeverity(Severity.INFO)
            .setBody(Semconv.CATALOG_PRODUCTS_SERVED)
            .setAllAttributes(Attributes.builder()
                .put("product.count", (long) productCount)
                .put("catalog.promo", promo)
                .build())
            .emit();
    }

    @GetMapping("/chaos/heap")
    @ResponseBody
    HeapPressureResult heapPressure(
        @RequestParam(defaultValue = "64") int mb,
        @RequestParam(defaultValue = "5000") long holdMs
    ) throws InterruptedException {
        int cappedMb = Math.max(0, Math.min(mb, MAX_HEAP_MB));
        long cappedHoldMs = Math.max(0, Math.min(holdMs, MAX_HEAP_HOLD_MS));
        Span.current().setAttribute("chaos.heap.requested_mb", mb);
        Span.current().setAttribute("chaos.heap.allocated_mb", cappedMb);
        Span.current().setAttribute("chaos.heap.hold_ms", cappedHoldMs);
        List<byte[]> held = new ArrayList<>(cappedMb);
        for (int i = 0; i < cappedMb; i++) {
            held.add(new byte[1024 * 1024]);
        }
        Thread.sleep(cappedHoldMs);
        return new HeapPressureResult(mb, cappedMb, holdMs, cappedHoldMs);
    }

    // A6: per-product `reviews` resolved via a @BatchMapping — Spring GraphQL
    // batches all products' review fetches into ONE DataLoader call, so the
    // trace shows a single batched fetch instead of an N+1 fan of per-product
    // calls. The OTel Java agent still emits one data-fetcher span per product
    // field, so we add a single scenario span around the actual batch fetch to
    // make the fetch pattern explicit and stable.
    @BatchMapping
    Map<Product, List<Review>> reviews(List<Product> products) {
        Span span = TRACER.spanBuilder("catalog.reviews.batch").startSpan();
        try (Scope ignored = span.makeCurrent()) {
            span.setAttribute("catalog.fetch_pattern", "batched");
            span.setAttribute("catalog.product.count", products.size());
            return products.stream().collect(Collectors.toMap(
                p -> p,
                this::reviewsFor
            ));
        } finally {
            span.end();
        }
    }

    // A6b: same data as `reviews`, intentionally fetched one product at a
    // time. With GraphQL data-fetcher spans on, Parallax shows the N+1 fan.
    @SchemaMapping(typeName = "Product", field = "reviewsSlow")
    List<Review> reviewsSlow(Product product) {
        Span.current().setAttribute("catalog.fetch_pattern", "n_plus_one");
        Span.current().setAttribute("catalog.product.sku", product.sku());
        Span span = TRACER.spanBuilder("catalog.reviews.single").startSpan();
        try (Scope ignored = span.makeCurrent()) {
            span.setAttribute("catalog.fetch_pattern", "n_plus_one");
            span.setAttribute("catalog.product.sku", product.sku());
            return reviewsFor(product);
        } finally {
            span.end();
        }
    }

    // Partial-error case: GraphQL returns HTTP 200 with errors[] and a null
    // field for one deterministic product.
    @SchemaMapping(typeName = "Product", field = "riskScore")
    Float riskScore(Product product) {
        Span.current().setAttribute("catalog.product.sku", product.sku());
        if (PARTIAL_ERROR_SKU.equals(product.sku())) {
            throw new IllegalStateException("risk score unavailable for " + PARTIAL_ERROR_SKU);
        }
        return product.priceMinor() > 3000 ? 0.72f : 0.18f;
    }

    private List<Review> reviewsFor(Product product) {
        return List.of(new Review("solid " + product.name(), 5), new Review("ok", 3));
    }

    // A7: GraphQL subscription — a long-lived streaming span. The data-fetcher
    // span stays open for the lifetime of the subscription (a known
    // backend-rendering weak spot), in contrast to the short request/response
    // spans above. Reached over the GraphQL-over-WebSocket transport.
    @SubscriptionMapping
    Flux<Product> priceChanges() {
        return Flux.interval(Duration.ofSeconds(1))
            .map(tick -> {
                Product base = CATALOG.get((int) (tick % CATALOG.size()));
                int jitter = (int) (tick % 5) * 10;
                return new Product(base.id(), base.sku(), base.name(),
                    base.priceMinor() + jitter);
            })
            .take(10);
    }
}

class FeatureFlagSpanEventHook implements BooleanHook {
    @Override
    public void after(
        HookContext<Boolean> ctx,
        FlagEvaluationDetails<Boolean> details,
        Map<String, Object> hints
    ) {
        Span.current().addEvent("feature_flag.evaluation", Attributes.builder()
            .put("feature_flag.key", ctx.getFlagKey())
            .put("feature_flag.provider_name", "flagd")
            .put("feature_flag.variant", Optional.ofNullable(details.getVariant()).orElse(""))
            .put("feature_flag.value", Boolean.TRUE.equals(details.getValue()))
            .build());
    }

    @Override
    public void error(HookContext<Boolean> ctx, Exception error, Map<String, Object> hints) {
        Span.current().addEvent("feature_flag.evaluation", Attributes.builder()
            .put("feature_flag.key", ctx.getFlagKey())
            .put("feature_flag.provider_name", "flagd")
            .put("feature_flag.variant", "error")
            .put("feature_flag.error", error.getClass().getSimpleName())
            .build());
    }
}

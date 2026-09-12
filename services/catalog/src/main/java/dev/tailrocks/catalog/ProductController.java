package dev.tailrocks.catalog;

import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.FlagEvaluationDetails;
import dev.openfeature.sdk.ImmutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Value;
import io.micrometer.core.instrument.Counter;
import io.micrometer.core.instrument.MeterRegistry;
import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.logs.Severity;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.Tracer;
import io.sentry.Sentry;
import io.tailrocks.semconv.Semconv;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.BatchMapping;
import org.springframework.graphql.data.method.annotation.MutationMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.graphql.data.method.annotation.SchemaMapping;
import org.springframework.graphql.data.method.annotation.SubscriptionMapping;
import graphql.GraphQLContext;
import graphql.schema.DataFetchingEnvironment;
import org.springframework.stereotype.Controller;
import reactor.core.publisher.Flux;
import reactor.core.publisher.Mono;
import reactor.core.scheduler.Schedulers;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.stream.Collectors;

@Controller
class ProductController {
    static final String DEFAULT_SEGMENT = "standard";
    static final String DEFAULT_SORT = "featured";
    private static final Logger LOG = LoggerFactory.getLogger(ProductController.class);
    private static final io.opentelemetry.api.logs.Logger EVENT_LOGGER =
        GlobalOpenTelemetry.get().getLogsBridge().get("catalog.events");
    private static final Tracer TRACER = GlobalOpenTelemetry.getTracer("catalog-graphql-scenarios");
    private static final String PRODUCTS_SERVED_EVENT = "catalog.products.served";
    private static final int MAX_PAGE_SIZE = 100;

    private final CatalogRepository catalog;
    private final CatalogCache cache;
    private final PriceChangeSource priceChanges;
    private final String syntheticRiskScoreFailureSku;
    private final Client flags = OpenFeatureAPI.getInstance().getClient("catalog");
    private final Counter productQueries;

    @Autowired
    ProductController(
        CatalogRepository catalog,
        CatalogCache cache,
        PriceChangeSource priceChanges,
        MeterRegistry meters,
        @org.springframework.beans.factory.annotation.Value(
            "${catalog.synthetic.risk-score-failure-sku:}"
        ) String syntheticRiskScoreFailureSku
    ) {
        this.catalog = catalog;
        this.cache = cache;
        this.priceChanges = priceChanges;
        this.syntheticRiskScoreFailureSku = syntheticRiskScoreFailureSku == null
            ? ""
            : syntheticRiskScoreFailureSku.trim();
        this.productQueries = Counter.builder(Semconv.CATALOG_PRODUCT_QUERIES)
            .description("catalog product page queries")
            .register(meters);
    }

    ProductController(
        CatalogRepository catalog,
        CatalogCache cache,
        PriceChangeSource priceChanges,
        MeterRegistry meters
    ) {
        this(catalog, cache, priceChanges, meters, "");
    }

    ProductController(CatalogRepository catalog, CatalogCache cache, MeterRegistry meters) {
        this(catalog, cache, (tenantId, sku) -> Flux.empty(), meters);
    }

    ProductController(CatalogRepository catalog, MeterRegistry meters) {
        this(catalog, CatalogCache.noop(), meters);
    }

    @QueryMapping
    Product product(
        @Argument String sku,
        @Argument String tenantId,
        @Argument String segment,
        DataFetchingEnvironment environment
    ) {
        return product(sku, tenantId, segment, environment.getGraphQlContext());
    }

    Product product(String sku, String tenantId, String segment) {
        return product(sku, tenantId, segment, GraphQLContext.getDefault());
    }

    Product product(
        String sku,
        String tenantId,
        String segment,
        GraphQLContext context
    ) {
        String tenant = CatalogRequestIdentity.resolve(context, tenantId);
        String customerSegment = normalizeSegment(segment);
        evaluateExperience(tenant, customerSegment);
        boolean noStore = CatalogGraphQlSecurityInterceptor.isNoStore(context);
        if (!noStore) {
            Optional<Product> cached = cache.getProduct(tenant, sku);
            if (cached.isPresent()) {
                Span.current().setAttribute("catalog.cache", "hit");
                return cached.get();
            }
            Span.current().setAttribute("catalog.cache", "miss");
        } else {
            Span.current().setAttribute("catalog.cache", "bypass");
        }
        Product product = catalog.findBySku(tenant, sku);
        if (!noStore && product != null) {
            cache.putProduct(tenant, sku, product);
        }
        return product;
    }

    @QueryMapping
    ProductPage products(
        @Argument String search,
        @Argument String category,
        @Argument String sort,
        @Argument String tenantId,
        @Argument Integer page,
        @Argument Integer size,
        @Argument String segment,
        DataFetchingEnvironment environment
    ) {
        return products(
            search, category, sort, tenantId, page, size, segment,
            environment.getGraphQlContext()
        );
    }

    ProductPage products(
        String search,
        String category,
        String sort,
        String tenantId,
        Integer page,
        Integer size,
        String segment
    ) {
        return products(
            search, category, sort, tenantId, page, size, segment,
            GraphQLContext.getDefault()
        );
    }

    ProductPage products(
        String search,
        String category,
        String sort,
        String tenantId,
        Integer page,
        Integer size,
        String segment,
        GraphQLContext context
    ) {
        String tenant = CatalogRequestIdentity.resolve(context, tenantId);
        String customerSegment = normalizeSegment(segment);
        String categorySlug = normalizeCategory(category);
        String sortOrder = normalizeSort(sort);
        int safePage = page == null ? 0 : Math.max(0, page);
        int safeSize = size == null ? 20 : Math.max(1, Math.min(size, MAX_PAGE_SIZE));
        String experience = evaluateExperience(tenant, customerSegment);
        String cacheKey = RedisCatalogCache.pageKey(
            tenant, search, categorySlug, sortOrder, safePage, safeSize,
            experience + ":" + customerSegment
        );
        boolean noStore = CatalogGraphQlSecurityInterceptor.isNoStore(context);

        productQueries.increment();
        if (!noStore) {
            Optional<ProductPage> cached = cache.getPage(cacheKey);
            if (cached.isPresent()) {
                Span.current().setAttribute("catalog.cache", "hit");
                ProductPage result = cached.get();
                emitCatalogProductsServed(tenant, result, experience);
                return result;
            }
            Span.current().setAttribute("catalog.cache", "miss");
        } else {
            Span.current().setAttribute("catalog.cache", "bypass");
        }
        ProductPage result = catalog.findProducts(
            tenant, search, categorySlug, sortOrder, safePage, safeSize, experience
        );
        if (!noStore) {
            cache.putPage(cacheKey, result);
        }
        emitCatalogProductsServed(tenant, result, experience);
        return result;
    }

    @QueryMapping
    List<Category> categories(
        @Argument String tenantId,
        DataFetchingEnvironment environment
    ) {
        return categories(tenantId, environment.getGraphQlContext());
    }

    List<Category> categories(String tenantId) {
        return categories(tenantId, GraphQLContext.getDefault());
    }

    List<Category> categories(String tenantId, GraphQLContext context) {
        return catalog.findCategories(CatalogRequestIdentity.resolve(context, tenantId));
    }

    @BatchMapping(typeName = "Product", field = "variants")
    Map<Product, List<ProductVariant>> variants(List<Product> products) {
        Map<Product, List<ProductVariant>> result = new LinkedHashMap<>();
        productsByTenant(products).forEach((tenantId, tenantProducts) -> {
            Map<String, List<ProductVariant>> loaded = catalog.findVariantsByProducts(
                tenantId,
                tenantProducts.stream().map(Product::id).distinct().toList()
            );
            tenantProducts.forEach(product -> result.put(
                product,
                loaded.getOrDefault(product.id(), List.of()).stream()
                    .filter(variant -> tenantId.equals(variant.tenantId()))
                    .filter(variant -> product.id().equals(variant.productId()))
                    .toList()
            ));
        });
        return result;
    }

    @BatchMapping(typeName = "Product", field = "reviews")
    Map<Product, List<Review>> reviews(List<Product> products) {
        Span span = TRACER.spanBuilder("catalog.reviews.batch").startSpan();
        try (var ignored = span.makeCurrent()) {
            span.setAttribute("catalog.fetch_pattern", "batched");
            span.setAttribute("catalog.product.count", products.size());
            Map<Product, List<Review>> result = new LinkedHashMap<>();
            productsByTenant(products).forEach((tenantId, tenantProducts) -> {
                Map<String, List<Review>> loaded = catalog.findReviewsByProducts(
                    tenantId,
                    tenantProducts.stream().map(Product::id).distinct().toList()
                );
                tenantProducts.forEach(product -> result.put(
                    product,
                    loaded.getOrDefault(product.id(), List.of()).stream()
                        .filter(review -> product.id().equals(review.productId()))
                        .toList()
                ));
            });
            return result;
        } finally {
            span.end();
        }
    }

    @SchemaMapping(typeName = "Product", field = "reviewsSlow")
    List<Review> reviewsSlow(Product product) {
        Span.current().setAttribute("catalog.fetch_pattern", "n_plus_one");
        Span.current().setAttribute("catalog.product.sku", product.sku());
        Span span = TRACER.spanBuilder("catalog.reviews.single").startSpan();
        try (var ignored = span.makeCurrent()) {
            span.setAttribute("catalog.fetch_pattern", "n_plus_one");
            span.setAttribute("catalog.product.sku", product.sku());
            return catalog.findReviews(product.tenantId(), product.id());
        } finally {
            span.end();
        }
    }

    @SchemaMapping(typeName = "Product", field = "riskScore")
    Float riskScore(Product product) {
        Span.current().setAttribute("catalog.product.sku", product.sku());
        if (!syntheticRiskScoreFailureSku.isEmpty()
            && syntheticRiskScoreFailureSku.equals(product.sku())) {
            IllegalStateException error =
                new IllegalStateException("risk score unavailable for " + product.sku());
            Sentry.captureException(error);
            throw error;
        }
        PriceSnapshot price = product.price();
        if (price == null || price.compareAtMinor() == null || price.compareAtMinor() <= 0) {
            return null;
        }
        // Price and compare-at amounts are loaded from the persisted price snapshot.
        // The resulting discount depth is a deterministic pricing-risk signal in [0, 1].
        return (float) (price.compareAtMinor() - price.amountMinor()) / price.compareAtMinor();
    }

    @MutationMapping
    Product updatePrice(
        @Argument PriceUpdateInput input,
        DataFetchingEnvironment environment
    ) {
        return updatePrice(input, environment.getGraphQlContext());
    }

    Product updatePrice(PriceUpdateInput input, GraphQLContext context) {
        CatalogAdminIdentity admin = CatalogAdminIdentity.require(context);
        if (input == null || input.sku() == null || input.sku().isBlank()) {
            throw new IllegalArgumentException("sku is required");
        }
        if (input.amountMinor() == null || input.amountMinor() < 0) {
            throw new IllegalArgumentException("amountMinor must be non-negative");
        }
        Integer compareAt = input.compareAtMinor();
        if (compareAt != null && compareAt < input.amountMinor()) {
            throw new IllegalArgumentException("compareAtMinor must be at least amountMinor");
        }
        String tenant = CatalogRequestIdentity.resolve(context, input.tenantId(), admin.tenantId());
        if (input.currency() == null || input.currency().isBlank()) {
            throw new IllegalArgumentException("currency is required");
        }
        String currency = input.currency().trim().toUpperCase(java.util.Locale.ROOT);
        Product updated = catalog.updatePrice(new PriceUpdateCommand(
            tenant,
            input.sku(),
            currency,
            input.amountMinor(),
            compareAt
        ));
        cache.evictProduct(tenant, input.sku());
        cache.evictTenant(tenant);
        return updated;
    }

    @SubscriptionMapping
    Flux<PriceChange> priceChanges(
        @Argument String tenantId,
        @Argument String sku,
        DataFetchingEnvironment environment
    ) {
        return priceChanges(tenantId, sku, environment.getGraphQlContext());
    }

    Flux<PriceChange> priceChanges(String tenantId, String sku) {
        return priceChanges(tenantId, sku, GraphQLContext.getDefault());
    }

    Flux<PriceChange> priceChanges(
        String tenantId,
        String sku,
        GraphQLContext context
    ) {
        String tenant = CatalogRequestIdentity.resolve(context, tenantId);
        String requestedSku = sku == null || sku.isBlank() ? null : sku;
        return priceChanges.stream(tenant, requestedSku)
            .flatMap(event -> Mono.fromCallable(() -> catalog.findPriceChange(event))
                .subscribeOn(Schedulers.boundedElastic()))
            .filter(Objects::nonNull);
    }

    private static Map<String, List<Product>> productsByTenant(List<Product> products) {
        return products.stream().collect(Collectors.groupingBy(
            ProductController::requiredTenant,
            LinkedHashMap::new,
            Collectors.toList()
        ));
    }

    private static String requiredTenant(Product product) {
        if (product.tenantId() == null || product.tenantId().isBlank()) {
            throw new IllegalArgumentException("product tenantId is required for batched resolution");
        }
        return product.tenantId();
    }

    private String evaluateExperience(String tenantId, String segment) {
        EvaluationContext context = new ImmutableContext(
            tenantId,
            Map.of(
                "tenant.id", new Value(tenantId),
                "customer.segment", new Value(segment)
            )
        );
        FlagEvaluationDetails<String> details = flags.getStringDetails(
            "catalogExperience",
            "standard",
            context
        );
        String variant = details.getValue() == null || details.getValue().isBlank()
            ? "standard"
            : details.getValue();
        Span.current().setAttribute("feature_flag.key", "catalogExperience");
        Span.current().setAttribute("feature_flag.variant", variant);
        Span.current().setAttribute("tenant.id", tenantId);
        Span.current().setAttribute("customer.segment", segment);
        return switch (variant) {
            case "featured", "newest", "standard" -> variant;
            default -> "standard";
        };
    }

    private static String normalizeSegment(String segment) {
        return segment == null || segment.isBlank() ? DEFAULT_SEGMENT : segment;
    }

    private static String normalizeCategory(String category) {
        if (category == null || category.isBlank()) {
            return null;
        }
        String normalized = category.trim().toLowerCase(java.util.Locale.ROOT);
        if (!normalized.matches("[a-z0-9]+(?:-[a-z0-9]+)*")) {
            throw new IllegalArgumentException("category must be a slug");
        }
        return normalized;
    }

    private static String normalizeSort(String sort) {
        String normalized = sort == null || sort.isBlank()
            ? DEFAULT_SORT
            : sort.trim().toLowerCase(java.util.Locale.ROOT);
        return switch (normalized) {
            case "featured", "newest", "price_asc", "price_desc", "relevance" -> normalized;
            default -> throw new IllegalArgumentException(
                "sort must be one of featured, newest, price_asc, price_desc, relevance"
            );
        };
    }

    private static void emitCatalogProductsServed(String tenantId, ProductPage page, String experience) {
        try (var ignoredTenant = org.slf4j.MDC.putCloseable("tenant.id", tenantId);
             var ignoredCount = org.slf4j.MDC.putCloseable("product.count", String.valueOf(page.items().size()));
             var ignoredExperience = org.slf4j.MDC.putCloseable("catalog.experience", experience)) {
            LOG.atInfo()
                .addKeyValue(Semconv.EVENT_NAME, PRODUCTS_SERVED_EVENT)
                .addKeyValue("tenant.id", tenantId)
                .addKeyValue("product.count", page.items().size())
                .addKeyValue("catalog.experience", experience)
                .log("catalog products served");
        }
        EVENT_LOGGER.logRecordBuilder()
            .setEventName(PRODUCTS_SERVED_EVENT)
            .setSeverity(Severity.INFO)
            .setBody(PRODUCTS_SERVED_EVENT)
            .setAllAttributes(Attributes.builder()
                .put("tenant.id", tenantId)
                .put("product.count", (long) page.items().size())
                .put("catalog.experience", experience)
                .build())
            .emit();
    }
}

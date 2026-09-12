package dev.tailrocks.catalog;

import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import graphql.GraphQLContext;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import reactor.core.publisher.Flux;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

@ExtendWith(OpenTelemetryTestExtension.class)
class ProductControllerTest {
    private static final Category CATEGORY = new Category(
        "cat-acme-kitchen", "tenant-acme", "kitchen", "Kitchen"
    );
    private static final Category OTHER_CATEGORY = new Category(
        "cat-other-kitchen", "tenant-other", "kitchen", "Kitchen"
    );
    private static final PriceSnapshot WIDGET_PRICE = new PriceSnapshot(
        "price-widget", "USD", 1999, 2299, "2026-01-03T10:00:00Z"
    );
    private static final Product WIDGET = new Product(
        "prod-acme-widget", "tenant-acme", "everyday-widget", "WIDGET-1",
        "Everyday Widget", "A dependable widget.", "Acme", CATEGORY, WIDGET_PRICE
    );
    private static final Product GADGET = new Product(
        "prod-acme-gadget", "tenant-acme", "smart-gadget", "GADGET-1",
        "Smart Gadget", "A connected gadget.", "Acme", CATEGORY,
        new PriceSnapshot("price-gadget", "USD", 4999, 5499, "2026-01-03T10:02:00Z")
    );
    private static final ProductVariant WIDGET_VARIANT = new ProductVariant(
        "var-acme-widget-1", "tenant-acme", WIDGET.id(), "WIDGET-1", "Standard",
        "{\"finish\":\"silver\"}", WIDGET_PRICE
    );
    private static final Review WIDGET_REVIEW = new Review(
        "review-widget", WIDGET.id(), "Solid", "Sturdy and useful.", 5, true,
        "2026-01-20T12:00:00Z"
    );
    private static final Product OTHER_WIDGET = new Product(
        "prod-other-widget", "tenant-other", "everyday-widget", "WIDGET-1",
        "Everyday Widget", "A tenant-specific widget.", "Other", OTHER_CATEGORY, WIDGET_PRICE
    );
    private static final ProductVariant OTHER_WIDGET_VARIANT = new ProductVariant(
        "var-other-widget-1", "tenant-other", OTHER_WIDGET.id(), "WIDGET-1", "Standard",
        "{\"finish\":\"black\"}", WIDGET_PRICE
    );
    private static final Review OTHER_WIDGET_REVIEW = new Review(
        "review-other-widget", OTHER_WIDGET.id(), "Solid", "Useful for this store.", 4, true,
        "2026-01-21T12:00:00Z"
    );

    private final FakeRepository repository = new FakeRepository();
    private final RecordingCache cache = new RecordingCache();
    private final FakePriceChangeSource priceChanges = new FakePriceChangeSource();
    private final ProductController controller = new ProductController(
        repository, cache, priceChanges, new SimpleMeterRegistry()
    );

    @Test
    void reads_shared_commerce_shape_and_preserves_batched_reviews() {
        assertEquals("Everyday Widget", controller.product("WIDGET-1", "tenant-acme", "standard").name());

        ProductPage page = controller.products("widget", "kitchen", "price_asc", "tenant-acme", 0, 10, "pro");
        assertEquals(2, page.totalElements());
        assertEquals("Kitchen", page.items().getFirst().category().name());

        Map<Product, List<ProductVariant>> variants = controller.variants(List.of(WIDGET));
        assertEquals("WIDGET-1", variants.get(WIDGET).getFirst().sku());

        Map<Product, List<Review>> reviews = controller.reviews(List.of(WIDGET));
        assertEquals(5, reviews.get(WIDGET).getFirst().stars());
        assertEquals("Sturdy and useful.", controller.reviewsSlow(WIDGET).getFirst().text());
    }

    @Test
    void batches_variants_and_reviews_per_parent_tenant() {
        List<Product> parents = List.of(WIDGET, OTHER_WIDGET);

        Map<Product, List<ProductVariant>> variants = controller.variants(parents);
        Map<Product, List<Review>> reviews = controller.reviews(parents);

        assertEquals(OTHER_WIDGET_VARIANT, variants.get(OTHER_WIDGET).getFirst());
        assertEquals(OTHER_WIDGET_REVIEW, reviews.get(OTHER_WIDGET).getFirst());
        assertEquals(List.of("tenant-acme", "tenant-other"), repository.variantTenantCalls);
        assertEquals(List.of("tenant-acme", "tenant-other"), repository.reviewTenantCalls);
    }

    @Test
    void product_query_uses_the_cache_before_postgres() {
        cache.cachedProduct = WIDGET;

        assertEquals(WIDGET, controller.product("WIDGET-1", "tenant-acme", "standard"));
        assertEquals(0, repository.productReads);
    }

    @Test
    void product_page_populates_cache_after_a_database_read() {
        ProductPage page = controller.products(null, null, "newest", "tenant-acme", 0, 10, "pro");

        assertEquals(1, repository.productPageReads);
        assertEquals(page, cache.cachedPage);
    }

    @Test
    void public_product_queries_reject_missing_tenant_identity() {
        assertThrows(IllegalArgumentException.class,
            () -> controller.product("WIDGET-1", null, "standard"));
    }

    @Test
    void public_product_queries_use_header_and_baggage_tenant_identity() {
        org.springframework.http.HttpHeaders headers = new org.springframework.http.HttpHeaders();
        headers.add("x-tenant-id", "tenant-acme");
        headers.add("baggage", "tenant.id=tenant-acme");
        GraphQLContext context = GraphQLContext.of(Map.of(
            CatalogRequestIdentity.CONTEXT_KEY,
            CatalogRequestIdentity.fromHeaders(headers)
        ));

        assertEquals(WIDGET, controller.product("WIDGET-1", null, "standard", context));
    }

    @Test
    void no_store_product_query_reads_postgres_without_reading_or_writing_cache() {
        cache.cachedProduct = GADGET;
        GraphQLContext context = GraphQLContext.of(Map.of(
            CatalogRequestIdentity.CONTEXT_KEY,
            CatalogRequestIdentity.fromHeaders(tenantHeaders()),
            CatalogGraphQlSecurityInterceptor.NO_STORE_CONTEXT_KEY,
            true
        ));

        assertEquals(WIDGET, controller.product("WIDGET-1", null, "standard", context));
        assertEquals(1, repository.productReads);
        assertEquals(0, cache.productGets);
        assertEquals(0, cache.productPuts);
        assertEquals(GADGET, cache.cachedProduct);
    }

    @Test
    void no_store_product_page_reads_postgres_without_reading_or_writing_cache() {
        cache.cachedPage = new ProductPage(List.of(GADGET), 0, 10, 1, "standard");
        GraphQLContext context = GraphQLContext.of(Map.of(
            CatalogRequestIdentity.CONTEXT_KEY,
            CatalogRequestIdentity.fromHeaders(tenantHeaders()),
            CatalogGraphQlSecurityInterceptor.NO_STORE_CONTEXT_KEY,
            true
        ));

        ProductPage page = controller.products(null, null, "newest", null, 0, 10, "standard", context);

        assertEquals(1, repository.productPageReads);
        assertEquals(0, cache.pageGets);
        assertEquals(0, cache.pagePuts);
        assertEquals(List.of(WIDGET, GADGET), page.items());
        assertEquals(1, cache.cachedPage.items().size());
    }

    @Test
    void computes_risk_score_from_the_persisted_price_snapshot() {
        assertEquals(500f / 5499f, controller.riskScore(GADGET));
    }

    @Test
    void risk_score_changes_when_persisted_price_fields_change() {
        Product repriced = new Product(
            GADGET.id(), GADGET.tenantId(), GADGET.slug(), GADGET.sku(), GADGET.name(),
            GADGET.description(), GADGET.brand(), GADGET.category(),
            new PriceSnapshot("price-gadget-repriced", "USD", 2500, 5000,
                "2026-01-03T10:02:00Z")
        );

        assertEquals(0.5f, controller.riskScore(repriced));
    }

    @Test
    void synthetic_risk_failure_requires_an_explicit_sku_knob() {
        ProductController scenarioController = new ProductController(
            repository,
            cache,
            priceChanges,
            new SimpleMeterRegistry(),
            "GADGET-1"
        );

        IllegalStateException error = assertThrows(IllegalStateException.class,
            () -> scenarioController.riskScore(GADGET));
        assertEquals("risk score unavailable for GADGET-1", error.getMessage());
    }

    @Test
    void leaves_risk_score_null_when_product_has_no_price() {
        Product unpriced = new Product(
            "prod-acme-unpriced", "tenant-acme", "unpriced", "NO-PRICE",
            "Unpriced", "No active price", "Acme", CATEGORY, null
        );

        assertNull(unpriced.getPriceMinor());
        assertNull(controller.riskScore(unpriced));
    }

    @Test
    void update_price_invalidates_product_and_tenant_page_cache() {
        Product updated = controller.updatePrice(new PriceUpdateInput(
            "tenant-acme", "WIDGET-1", "usd", 2199, 2499
        ), adminContext("tenant-acme"));

        assertNotNull(updated);
        assertEquals("WIDGET-1", updated.sku());
        assertEquals("WIDGET-1", cache.evictedSku);
        assertEquals("tenant-acme", cache.evictedTenant);
        assertEquals("tenant-acme", repository.lastPriceUpdate.tenantId());
    }

    @Test
    void update_price_derives_tenant_from_the_authenticated_admin_identity() {
        controller.updatePrice(new PriceUpdateInput(null, "WIDGET-1", "USD", 2199, 2499),
            adminContext("tenant-other"));

        assertEquals("tenant-other", repository.lastPriceUpdate.tenantId());
        assertEquals("tenant-other", cache.evictedTenant);
    }

    @Test
    void update_price_rejects_a_tenant_different_from_the_authenticated_admin() {
        assertThrows(IllegalArgumentException.class, () -> controller.updatePrice(
            new PriceUpdateInput("tenant-other", "WIDGET-1", "USD", 2199, 2499),
            adminContext("tenant-acme")
        ));

        assertNull(repository.lastPriceUpdate);
        assertNull(cache.evictedTenant);
    }

    @Test
    void update_price_requires_an_authenticated_catalog_admin() {
        assertThrows(IllegalStateException.class, () -> controller.updatePrice(
            new PriceUpdateInput("tenant-acme", "WIDGET-1", "USD", 2199, 2499),
            GraphQLContext.getDefault()
        ));
    }

    @Test
    void update_price_requires_currency_instead_of_fabricating_usd() {
        assertThrows(IllegalArgumentException.class, () -> controller.updatePrice(
            new PriceUpdateInput("tenant-acme", "WIDGET-1", null, 2199, 2499),
            adminContext("tenant-acme")
        ));

        assertNull(repository.lastPriceUpdate);
        assertNull(cache.evictedTenant);
    }

    @Test
    void subscription_emits_the_committed_notification_without_a_synthetic_initial_value() {
        priceChanges.events = Flux.just(new PriceChangeEvent(
            "tenant-acme", "WIDGET-1", WIDGET.id(), WIDGET_VARIANT.id(),
            "price-committed", WIDGET_PRICE.currency(), 2199, 2499,
            "2026-09-04T00:00:00Z", "2026-09-04T00:00:00Z",
            "event-committed", 1
        ));

        PriceChange change = controller.priceChanges("tenant-acme", "WIDGET-1")
            .take(1)
            .blockFirst(Duration.ofSeconds(3));

        assertNotNull(change);
        assertEquals(2199, change.price().amountMinor());
        assertEquals("2026-09-04T00:00:00Z", change.observedAt());
        assertEquals("WIDGET-1", change.variant().sku());
    }

    private static final class FakeRepository implements CatalogRepository {
        private int productReads;
        private int productPageReads;
        private final List<String> variantTenantCalls = new ArrayList<>();
        private final List<String> reviewTenantCalls = new ArrayList<>();
        private PriceUpdateCommand lastPriceUpdate;

        @Override
        public Product findBySku(String tenantId, String sku) {
            productReads++;
            return "WIDGET-1".equals(sku) ? WIDGET : "GADGET-1".equals(sku) ? GADGET : null;
        }

        @Override
        public ProductPage findProducts(
            String tenantId,
            String search,
            String category,
            String sort,
            int page,
            int size,
            String experience
        ) {
            productPageReads++;
            return new ProductPage(List.of(WIDGET, GADGET), page, size, 2, experience);
        }

        @Override
        public Map<String, List<ProductVariant>> findVariantsByProducts(String tenantId, List<String> productIds) {
            variantTenantCalls.add(tenantId);
            return "tenant-other".equals(tenantId)
                ? Map.of(OTHER_WIDGET.id(), List.of(OTHER_WIDGET_VARIANT))
                : Map.of(WIDGET.id(), List.of(WIDGET_VARIANT));
        }

        @Override
        public Map<String, List<Review>> findReviewsByProducts(String tenantId, List<String> productIds) {
            reviewTenantCalls.add(tenantId);
            return "tenant-other".equals(tenantId)
                ? Map.of(OTHER_WIDGET.id(), List.of(OTHER_WIDGET_REVIEW))
                : Map.of(WIDGET.id(), List.of(WIDGET_REVIEW));
        }

        @Override
        public List<Review> findReviews(String tenantId, String productId) {
            return WIDGET.id().equals(productId) ? List.of(WIDGET_REVIEW) : List.of();
        }

        @Override
        public List<Category> findCategories(String tenantId) {
            return List.of(CATEGORY);
        }

        @Override
        public Product updatePrice(PriceUpdateCommand command) {
            lastPriceUpdate = command;
            return WIDGET;
        }

        @Override
        public PriceChange findPriceChange(PriceChangeEvent event) {
            PriceSnapshot price = new PriceSnapshot(
                event.priceId(), event.currency(), event.amountMinor(),
                event.compareAtMinor(), event.validFrom()
            );
            ProductVariant variant = new ProductVariant(
                WIDGET_VARIANT.id(), WIDGET_VARIANT.tenantId(), WIDGET_VARIANT.productId(),
                WIDGET_VARIANT.sku(), WIDGET_VARIANT.name(), WIDGET_VARIANT.options(), price
            );
            return new PriceChange(WIDGET, variant, price, event.observedAt());
        }
    }

    private static final class FakePriceChangeSource implements PriceChangeSource {
        private Flux<PriceChangeEvent> events = Flux.empty();

        @Override
        public Flux<PriceChangeEvent> stream(String tenantId, String sku) {
            return events;
        }
    }

    private static final class RecordingCache implements CatalogCache {
        private Product cachedProduct;
        private ProductPage cachedPage;
        private int productGets;
        private int productPuts;
        private int pageGets;
        private int pagePuts;
        private String evictedSku;
        private String evictedTenant;

        @Override
        public Optional<Product> getProduct(String tenantId, String sku) {
            productGets++;
            return Optional.ofNullable(cachedProduct);
        }

        @Override
        public void putProduct(String tenantId, String sku, Product product) {
            productPuts++;
            cachedProduct = product;
        }

        @Override
        public Optional<ProductPage> getPage(String key) {
            pageGets++;
            return Optional.ofNullable(cachedPage);
        }

        @Override
        public void putPage(String key, ProductPage page) {
            pagePuts++;
            cachedPage = page;
        }

        @Override
        public void evictProduct(String tenantId, String sku) {
            evictedSku = sku;
        }

        @Override
        public void evictTenant(String tenantId) {
            evictedTenant = tenantId;
        }
    }

    private static GraphQLContext adminContext(String tenantId) {
        return GraphQLContext.of(Map.of(
            CatalogAdminIdentity.CONTEXT_KEY,
            new CatalogAdminIdentity(tenantId)
        ));
    }

    private static org.springframework.http.HttpHeaders tenantHeaders() {
        org.springframework.http.HttpHeaders headers = new org.springframework.http.HttpHeaders();
        headers.add("x-tenant-id", "tenant-acme");
        headers.add("baggage", "tenant.id=tenant-acme");
        return headers;
    }
}

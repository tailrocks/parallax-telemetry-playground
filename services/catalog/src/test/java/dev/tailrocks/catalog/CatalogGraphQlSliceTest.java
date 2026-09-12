package dev.tailrocks.catalog;

import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.graphql.test.autoconfigure.GraphQlTest;
import org.springframework.boot.micrometer.tracing.test.autoconfigure.AutoConfigureTracing;
import org.springframework.boot.test.context.TestConfiguration;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Import;
import org.springframework.graphql.test.tester.GraphQlTester;
import org.springframework.test.context.bean.override.mockito.MockitoBean;

import java.util.List;
import java.util.Map;
import java.util.Optional;

import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyInt;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.ArgumentMatchers.nullable;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.when;

@GraphQlTest(controllers = ProductController.class)
@AutoConfigureTracing
@Import(CatalogGraphQlSliceTest.Meters.class)
@ExtendWith(OpenTelemetryTestExtension.class)
class CatalogGraphQlSliceTest {
    private static final Category CATEGORY = new Category(
        "cat-acme-kitchen", "tenant-acme", "kitchen", "Kitchen"
    );
    private static final PriceSnapshot PRICE = new PriceSnapshot(
        "price-widget", "USD", 1999, 2299, "2026-01-03T10:00:00Z"
    );
    private static final Product WIDGET = new Product(
        "prod-acme-widget", "tenant-acme", "everyday-widget", "WIDGET-1",
        "Everyday Widget", "A dependable widget.", "Acme", CATEGORY, PRICE
    );
    private static final Product GADGET = new Product(
        "prod-acme-gadget", "tenant-acme", "smart-gadget", "GADGET-1",
        "Smart Gadget", "A connected gadget.", "Acme", CATEGORY,
        new PriceSnapshot("price-gadget", "USD", 4999, 5499, "2026-01-03T10:02:00Z")
    );
    private static final Product NO_PRICE = new Product(
        "prod-acme-no-price", "tenant-acme", "unpriced-product", "NO-PRICE",
        "Unpriced Product", "A product without an active price.", "Acme", CATEGORY, null
    );
    private static final ProductVariant VARIANT = new ProductVariant(
        "var-acme-widget-1", "tenant-acme", WIDGET.id(), "WIDGET-1", "Standard",
        "{\"finish\":\"silver\"}", PRICE
    );
    private static final Review REVIEW = new Review(
        "review-widget", WIDGET.id(), "Solid", "Sturdy and useful.", 5, true,
        "2026-01-20T12:00:00Z"
    );

    @Autowired
    private GraphQlTester graphQlTester;

    @MockitoBean
    private CatalogRepository catalog;

    @MockitoBean
    private CatalogCache cache;

    @MockitoBean
    private PriceChangeSource priceChanges;

    @BeforeEach
    void setUp() {
        when(catalog.findProducts(
            anyString(), nullable(String.class), nullable(String.class), nullable(String.class),
            anyInt(), anyInt(), anyString()
        ))
            .thenReturn(new ProductPage(List.of(WIDGET, GADGET), 0, 20, 2, "standard"));
        when(catalog.findBySku("tenant-acme", "WIDGET-1")).thenReturn(WIDGET);
        when(catalog.findBySku("tenant-acme", "NO-PRICE")).thenReturn(NO_PRICE);
        when(catalog.findVariantsByProducts(anyString(), any())).thenReturn(
            Map.of(WIDGET.id(), List.of(VARIANT))
        );
        when(catalog.findReviewsByProducts(anyString(), any())).thenReturn(
            Map.of(WIDGET.id(), List.of(REVIEW))
        );
        when(catalog.findReviews("tenant-acme", WIDGET.id())).thenReturn(List.of(REVIEW));
        when(cache.getPage(anyString())).thenReturn(java.util.Optional.empty());
        when(cache.getProduct(anyString(), anyString())).thenReturn(java.util.Optional.empty());
    }

    @Test
    void serves_paged_search_with_category_variant_and_batched_review_fields() {
        graphQlTester.document("""
                { products(tenantId: "tenant-acme", search: "widget", category: "kitchen", sort: PRICE_ASC,
                    page: 0, size: 20, segment: "pro") {
                    items { sku category { slug } variants { sku price { amountMinor } }
                      reviews { stars text } reviewsSlow { text } }
                    totalElements totalPages hasNext
                  } }
                """)
            .execute()
            .path("products.items[0].sku").entity(String.class).isEqualTo("WIDGET-1")
            .path("products.items[0].category.slug").entity(String.class).isEqualTo("kitchen")
            .path("products.items[0].variants[0].price.amountMinor").entity(Integer.class).isEqualTo(1999)
            .path("products.items[0].reviews[0].stars").entity(Integer.class).isEqualTo(5)
            .path("products.totalElements").entity(Integer.class).isEqualTo(2)
            .path("products.hasNext").entity(Boolean.class).isEqualTo(false);
    }

    @Test
    void serves_a_cached_product_page_without_repeating_the_database_query() {
        when(cache.getPage(anyString())).thenReturn(Optional.of(
            new ProductPage(List.of(WIDGET), 0, 20, 1, "standard")
        ));

        graphQlTester.document("{ products(tenantId: \"tenant-acme\", search: \"widget\") { items { sku category { name } } } }")
            .execute()
            .path("products.items[0].sku").entity(String.class).isEqualTo("WIDGET-1")
            .path("products.items[0].category.name").entity(String.class).isEqualTo("Kitchen");

        verify(catalog, never()).findProducts(
            anyString(), nullable(String.class), nullable(String.class), nullable(String.class),
            anyInt(), anyInt(), anyString()
        );
    }

    @Test
    void serves_the_persisted_price_derived_risk_score_without_a_normal_path_failure() {
        graphQlTester.document("{ products(tenantId: \"tenant-acme\") { items { sku riskScore } } }")
            .execute()
            .errors().verify()
            .path("products.items[1].riskScore")
            .entity(Float.class)
            .isEqualTo(500f / 5499f);
    }

    @Test
    void exposes_null_price_and_risk_score_for_an_unpriced_product() {
        graphQlTester.document("""
                { product(tenantId: "tenant-acme", sku: "NO-PRICE") { sku priceMinor price { amountMinor } riskScore } }
                """)
            .execute()
            .path("product.sku").entity(String.class).isEqualTo("NO-PRICE")
            .path("product.priceMinor").valueIsNull()
            .path("product.price").valueIsNull()
            .path("product.riskScore").valueIsNull();
    }

    @Test
    void update_price_rejects_an_unauthenticated_public_call() {
        when(catalog.updatePrice(any())).thenReturn(WIDGET);

        graphQlTester.document("""
                mutation { updatePrice(input: {
                  tenantId: "tenant-acme", sku: "WIDGET-1", currency: "USD", amountMinor: 2199
                }) { sku price { amountMinor } } }
                """)
            .execute()
            .errors()
            .satisfy(errors -> Assertions.assertFalse(errors.isEmpty()));

        verify(catalog, never()).updatePrice(any());
        verify(cache, never()).evictProduct(anyString(), anyString());
        verify(cache, never()).evictTenant(anyString());
    }

    @Test
    void update_price_requires_currency_at_the_graphql_schema_boundary() {
        graphQlTester.document("""
                mutation { updatePrice(input: {
                  tenantId: "tenant-acme", sku: "WIDGET-1", amountMinor: 2199
                }) { sku } }
                """)
            .execute()
            .errors()
            .satisfy(errors -> Assertions.assertFalse(errors.isEmpty()));

        verify(catalog, never()).updatePrice(any());
    }

    @TestConfiguration(proxyBeanMethods = false)
    static class Meters {
        @Bean
        MeterRegistry meterRegistry() {
            return new SimpleMeterRegistry();
        }
    }
}

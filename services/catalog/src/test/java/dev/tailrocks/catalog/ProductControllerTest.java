package dev.tailrocks.catalog;

import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

class ProductControllerTest {
    private static final List<Product> PRODUCTS = List.of(
        new Product("1", "WIDGET-1", "Widget", 1999),
        new Product("2", "GADGET-1", "Gadget", 4999)
    );
    private final ProductController controller = new ProductController(new CatalogRepository() {
        @Override
        public Product findBySku(String sku) {
            return PRODUCTS.stream().filter(product -> product.sku().equals(sku)).findFirst().orElse(null);
        }

        @Override
        public List<Product> findAll() {
            return PRODUCTS;
        }
    }, new SimpleMeterRegistry());

    @Test
    void resolvesProductsAndPreservesBatchReviewShape() {
        Product widget = controller.product("WIDGET-1");
        assertEquals("Widget", widget.name());
        assertNull(controller.product("UNKNOWN"));

        Map<Product, List<Review>> reviews = controller.reviews(List.of(widget));
        assertEquals(2, reviews.get(widget).size());
        assertEquals(5, reviews.get(widget).getFirst().stars());
    }

    @Test
    void retainsDeterministicPartialErrorAndNPlusOneField() {
        Product gadget = controller.product("GADGET-1");
        assertEquals(2, controller.reviewsSlow(gadget).size());
        try {
            controller.riskScore(gadget);
        } catch (IllegalStateException error) {
            assertEquals("risk score unavailable for GADGET-1", error.getMessage());
            return;
        }
        throw new AssertionError("GADGET-1 must produce a partial GraphQL error");
    }
}

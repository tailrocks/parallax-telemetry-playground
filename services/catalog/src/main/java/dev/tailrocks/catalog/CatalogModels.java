package dev.tailrocks.catalog;

import java.util.List;
import java.util.Map;

record Category(String id, String tenantId, String slug, String name) {}

record PriceSnapshot(
    String id,
    String currency,
    int amountMinor,
    Integer compareAtMinor,
    String validFrom
) {}

record ProductVariant(
    String id,
    String tenantId,
    String productId,
    String sku,
    String name,
    String options,
    PriceSnapshot price
) {}

record Product(
    String id,
    String tenantId,
    String slug,
    String sku,
    String name,
    String description,
    String brand,
    Category category,
    PriceSnapshot price
) {
    Integer getPriceMinor() {
        return price == null ? null : price.amountMinor();
    }
}

record ProductPage(List<Product> items, int page, int size, long totalElements, String experience) {
    int getTotalPages() {
        return size == 0 ? 0 : (int) ((totalElements + size - 1) / size);
    }

    boolean isHasNext() {
        return (long) (page + 1) * size < totalElements;
    }
}

record Review(
    String id,
    String productId,
    String title,
    String text,
    int stars,
    boolean verifiedPurchase,
    String createdAt
) {}

record PriceChange(Product product, ProductVariant variant, PriceSnapshot price, String observedAt) {}

record PriceChangeEvent(
    String tenantId,
    String sku,
    String productId,
    String variantId,
    String priceId,
    String currency,
    int amountMinor,
    Integer compareAtMinor,
    String validFrom,
    String observedAt,
    String eventId,
    long sequence
) {
    static final String CHANNEL = "price_changes";
}

record PriceUpdateInput(
    String tenantId,
    String sku,
    String currency,
    Integer amountMinor,
    Integer compareAtMinor
) {}

record PriceUpdateCommand(
    String tenantId,
    String sku,
    String currency,
    int amountMinor,
    Integer compareAtMinor
) {}

record FeatureContext(String tenantId, String segment) {
    Map<String, Object> attributes() {
        return Map.of(
            "tenant.id", tenantId,
            "customer.segment", segment
        );
    }
}

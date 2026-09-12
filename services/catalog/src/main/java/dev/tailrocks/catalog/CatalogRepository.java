package dev.tailrocks.catalog;

import java.util.List;
import java.util.Map;

interface CatalogRepository {
    Product findBySku(String tenantId, String sku);

    ProductPage findProducts(
        String tenantId,
        String search,
        String category,
        String sort,
        int page,
        int size,
        String experience
    );

    Map<String, List<ProductVariant>> findVariantsByProducts(String tenantId, List<String> productIds);

    Map<String, List<Review>> findReviewsByProducts(String tenantId, List<String> productIds);

    List<Review> findReviews(String tenantId, String productId);

    List<Category> findCategories(String tenantId);

    Product updatePrice(PriceUpdateCommand command);

    PriceChange findPriceChange(PriceChangeEvent event);
}

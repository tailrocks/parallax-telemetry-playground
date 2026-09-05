package dev.tailrocks.catalog;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.springframework.jdbc.core.simple.JdbcClient;
import org.springframework.stereotype.Repository;
import org.springframework.transaction.annotation.Transactional;

import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Timestamp;
import java.time.Instant;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

@Repository
class JdbcCatalogRepository implements CatalogRepository {
    private static final String PRODUCT_COLUMNS = """
        p.id AS product_id,
        p.tenant_id,
        p.slug,
        p.name AS product_name,
        p.description,
        p.brand,
        c.id AS category_id,
        c.slug AS category_slug,
        c.name AS category_name,
        pv.sku AS product_sku,
        pr.id AS price_id,
        pr.currency AS price_currency,
        pr.amount_minor,
        pr.compare_at_minor,
        pr.valid_from AS price_valid_from
        """;

    private static final String PRODUCT_FROM = """
        FROM products p
        JOIN tenants t ON t.id = p.tenant_id
        JOIN categories c ON c.tenant_id = p.tenant_id AND c.id = p.category_id
        LEFT JOIN LATERAL (
            SELECT pv.id, pv.sku
            FROM product_variants pv
            WHERE pv.tenant_id = p.tenant_id
              AND pv.product_id = p.id
              AND pv.status = 'active'
              AND (CAST(:sku AS TEXT) IS NULL OR pv.sku = CAST(:sku AS TEXT))
            ORDER BY CASE WHEN CAST(:sku AS TEXT) IS NULL THEN 1 ELSE 0 END, pv.id
            LIMIT 1
        ) pv ON TRUE
        LEFT JOIN LATERAL (
            SELECT pr.id, pr.currency, pr.amount_minor, pr.compare_at_minor, pr.valid_from
            FROM prices pr
            WHERE pr.tenant_id = p.tenant_id
              AND pr.variant_id = pv.id
              AND pr.currency = t.default_currency
              AND pr.is_default = TRUE
              AND pr.valid_from <= CURRENT_TIMESTAMP
              AND (pr.valid_to IS NULL OR pr.valid_to > CURRENT_TIMESTAMP)
            ORDER BY pr.valid_from DESC
            LIMIT 1
        ) pr ON TRUE
        """;

    private final JdbcClient jdbc;
    private final ObjectMapper objectMapper;

    JdbcCatalogRepository(JdbcClient jdbc, ObjectMapper objectMapper) {
        this.jdbc = jdbc;
        this.objectMapper = objectMapper;
    }

    @Override
    public Product findBySku(String tenantId, String sku) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        return jdbc.sql("""
                SELECT %s
                %s
                WHERE p.tenant_id = :tenantId
                  AND p.status = 'active'
                  AND EXISTS (
                      SELECT 1
                      FROM product_variants requested_variant
                      WHERE requested_variant.tenant_id = p.tenant_id
                        AND requested_variant.product_id = p.id
                        AND requested_variant.sku = :sku
                        AND requested_variant.status = 'active'
                  )
                """.formatted(PRODUCT_COLUMNS, PRODUCT_FROM))
            .param("tenantId", tenantId)
            .param("sku", sku)
            .query(JdbcCatalogRepository::mapProduct)
            .optional()
            .orElse(null);
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
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        String normalizedSearch = search == null ? "" : search.trim();
        String pattern = "%" + normalizedSearch + "%";
        long total = jdbc.sql("""
                SELECT COUNT(*)
                FROM products p
                JOIN categories c ON c.tenant_id = p.tenant_id AND c.id = p.category_id
                WHERE p.tenant_id = :tenantId
                  AND p.status = 'active'
                  AND (CAST(:category AS TEXT) IS NULL OR c.slug = CAST(:category AS TEXT))
                  AND (
                      :search = ''
                      OR p.name ILIKE :pattern
                      OR p.slug ILIKE :pattern
                      OR p.description ILIKE :pattern
                      OR EXISTS (
                          SELECT 1
                          FROM product_variants pv_search
                          WHERE pv_search.tenant_id = p.tenant_id
                            AND pv_search.product_id = p.id
                            AND pv_search.sku ILIKE :pattern
                      )
                  )
                """)
            .param("tenantId", tenantId)
            .param("category", category)
            .param("search", normalizedSearch)
            .param("pattern", pattern)
            .query((resultSet, rowNum) -> resultSet.getLong(1))
            .single();

        String orderBy = switch (sort) {
            case "featured" -> "(p.attributes ->> 'featured')::BOOLEAN DESC, p.created_at DESC, p.id";
            case "newest" -> "p.created_at DESC, p.id";
            case "price_asc" -> "pr.amount_minor ASC NULLS LAST, p.id";
            case "price_desc" -> "pr.amount_minor DESC NULLS LAST, p.id";
            default -> "p.id";
        };

        List<Product> items = jdbc.sql("""
                SELECT %s
                %s
                WHERE p.tenant_id = :tenantId
                  AND p.status = 'active'
                  AND (CAST(:category AS TEXT) IS NULL OR c.slug = CAST(:category AS TEXT))
                  AND (
                      :search = ''
                      OR p.name ILIKE :pattern
                      OR p.slug ILIKE :pattern
                      OR p.description ILIKE :pattern
                      OR EXISTS (
                          SELECT 1
                          FROM product_variants pv_search
                          WHERE pv_search.tenant_id = p.tenant_id
                            AND pv_search.product_id = p.id
                            AND pv_search.sku ILIKE :pattern
                      )
                  )
                ORDER BY %s
                LIMIT :size OFFSET :offset
                """.formatted(PRODUCT_COLUMNS, PRODUCT_FROM, orderBy))
            .param("tenantId", tenantId)
            .param("sku", null)
            .param("category", category)
            .param("search", normalizedSearch)
            .param("pattern", pattern)
            .param("size", size)
            .param("offset", page * size)
            .query(JdbcCatalogRepository::mapProduct)
            .list();

        return new ProductPage(items, page, size, total, experience);
    }

    @Override
    public Map<String, List<ProductVariant>> findVariantsByProducts(String tenantId, List<String> productIds) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        if (productIds.isEmpty()) {
            return Map.of();
        }
        String placeholders = namedPlaceholders(productIds.size());
        String sql = """
            SELECT
                pv.id AS variant_id,
                pv.tenant_id,
                pv.product_id,
                pv.sku,
                pv.name AS variant_name,
                pv.option_values::TEXT AS options,
                pr.id AS price_id,
                pr.currency AS price_currency,
                pr.amount_minor,
                pr.compare_at_minor,
                pr.valid_from AS price_valid_from
            FROM product_variants pv
            JOIN tenants t ON t.id = pv.tenant_id
            LEFT JOIN LATERAL (
                SELECT pr.id, pr.currency, pr.amount_minor, pr.compare_at_minor, pr.valid_from
                FROM prices pr
                WHERE pr.tenant_id = pv.tenant_id
                  AND pr.variant_id = pv.id
                  AND pr.currency = t.default_currency
                  AND pr.is_default = TRUE
                  AND pr.valid_from <= CURRENT_TIMESTAMP
                  AND (pr.valid_to IS NULL OR pr.valid_to > CURRENT_TIMESTAMP)
                ORDER BY pr.valid_from DESC
                LIMIT 1
            ) pr ON TRUE
            WHERE pv.tenant_id = :tenantId
              AND pv.product_id IN (%s)
              AND pv.status = 'active'
            ORDER BY pv.product_id, pv.id
            """.formatted(placeholders);

        var query = jdbc.sql(sql).param("tenantId", tenantId);
        for (int index = 0; index < productIds.size(); index++) {
            query = query.param("productId" + index, productIds.get(index));
        }
        Map<String, List<ProductVariant>> variants = new LinkedHashMap<>();
        query.query((resultSet, rowNum) -> mapVariant(resultSet, rowNum))
            .list()
            .forEach(variant -> variants.computeIfAbsent(variant.productId(), ignored -> new ArrayList<>()).add(variant));
        return variants;
    }

    @Override
    public Map<String, List<Review>> findReviewsByProducts(String tenantId, List<String> productIds) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        if (productIds.isEmpty()) {
            return Map.of();
        }
        String placeholders = namedPlaceholders(productIds.size());
        String sql = """
            SELECT id, product_id, title, body, rating, verified_purchase, created_at
            FROM reviews
            WHERE tenant_id = :tenantId
              AND product_id IN (%s)
              AND status = 'published'
            ORDER BY product_id, created_at DESC, id
            """.formatted(placeholders);
        var query = jdbc.sql(sql).param("tenantId", tenantId);
        for (int index = 0; index < productIds.size(); index++) {
            query = query.param("productId" + index, productIds.get(index));
        }
        Map<String, List<Review>> reviews = new LinkedHashMap<>();
        query.query((resultSet, rowNum) -> mapReview(resultSet, rowNum))
            .list()
            .forEach(review -> reviews.computeIfAbsent(review.productId(), ignored -> new ArrayList<>()).add(review));
        return reviews;
    }

    @Override
    public List<Review> findReviews(String tenantId, String productId) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        return jdbc.sql("""
                SELECT id, product_id, title, body, rating, verified_purchase, created_at
                FROM reviews
                WHERE tenant_id = :tenantId
                  AND product_id = :productId
                  AND status = 'published'
                ORDER BY created_at DESC, id
                """)
            .param("tenantId", tenantId)
            .param("productId", productId)
            .query(JdbcCatalogRepository::mapReview)
            .list();
    }

    @Override
    public List<Category> findCategories(String tenantId) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        return jdbc.sql("""
                SELECT id, tenant_id, slug, name
                FROM categories
                WHERE tenant_id = :tenantId
                ORDER BY sort_order, id
                """)
            .param("tenantId", tenantId)
            .query((resultSet, rowNum) -> new Category(
                resultSet.getString("id"),
                resultSet.getString("tenant_id"),
                resultSet.getString("slug"),
                resultSet.getString("name")
            ))
            .list();
    }

    @Override
    @Transactional
    public Product updatePrice(PriceUpdateCommand command) {
        String tenantId = CatalogRequestIdentity.requireTenantValue(command.tenantId());
        String currency = command.currency() == null
            ? ""
            : command.currency().trim().toUpperCase(java.util.Locale.ROOT);
        if (currency.isBlank()) {
            throw new IllegalArgumentException("currency is required");
        }
        String tenantCurrency = jdbc.sql("""
                SELECT default_currency
                FROM tenants
                WHERE id = :tenantId
                FOR SHARE
                """)
            .param("tenantId", tenantId)
            .query((resultSet, rowNum) -> resultSet.getString("default_currency"))
            .optional()
            .orElseThrow(() -> new IllegalArgumentException("unknown tenant: " + tenantId));
        if (!tenantCurrency.equals(currency)) {
            throw new IllegalArgumentException(
                "currency does not match tenant default currency: " + tenantCurrency
            );
        }
        VariantReference variant = jdbc.sql("""
                SELECT id, product_id
                FROM product_variants
                WHERE tenant_id = :tenantId
                  AND sku = :sku
                  AND status = 'active'
                FOR UPDATE
                """)
            .param("tenantId", tenantId)
            .param("sku", command.sku())
            .query((resultSet, rowNum) -> new VariantReference(
                resultSet.getString("id"),
                resultSet.getString("product_id")
            ))
            .optional()
            .orElseThrow(() -> new IllegalArgumentException("unknown active SKU: " + command.sku()));

        Instant now = jdbc.sql("SELECT CURRENT_TIMESTAMP")
            .query((resultSet, rowNum) -> resultSet.getTimestamp(1).toInstant())
            .single();
        Timestamp timestamp = Timestamp.from(now);
        String priceId = "price-" + UUID.randomUUID();
        jdbc.sql("""
                UPDATE prices
                SET is_default = FALSE, valid_to = :validTo
                WHERE tenant_id = :tenantId
                  AND variant_id = :variantId
                  AND currency = :currency
                  AND is_default = TRUE
                """)
            .param("validTo", timestamp)
            .param("tenantId", tenantId)
            .param("variantId", variant.id())
            .param("currency", currency)
            .update();

        jdbc.sql("""
                INSERT INTO prices
                    (id, tenant_id, variant_id, currency, amount_minor, compare_at_minor,
                     valid_from, valid_to, is_default, created_at)
                VALUES
                    (:id, :tenantId, :variantId, :currency, :amountMinor, :compareAtMinor,
                     :validFrom, NULL, TRUE, :createdAt)
                """)
            .param("id", priceId)
            .param("tenantId", tenantId)
            .param("variantId", variant.id())
            .param("currency", currency)
            .param("amountMinor", command.amountMinor())
            .param("compareAtMinor", command.compareAtMinor())
            .param("validFrom", timestamp)
            .param("createdAt", timestamp)
            .update();

        String eventId = "price-change-" + UUID.randomUUID();
        long sequence = jdbc.sql("""
                INSERT INTO price_change_events
                    (event_id, tenant_id, sku, product_id, variant_id, price_id,
                     currency, amount_minor, compare_at_minor, valid_from, observed_at)
                VALUES
                    (:eventId, :tenantId, :sku, :productId, :variantId, :priceId,
                     :currency, :amountMinor, :compareAtMinor, :validFrom, :observedAt)
                RETURNING sequence
                """)
            .param("eventId", eventId)
            .param("tenantId", tenantId)
            .param("sku", command.sku())
            .param("productId", variant.productId())
            .param("variantId", variant.id())
            .param("priceId", priceId)
            .param("currency", currency)
            .param("amountMinor", command.amountMinor())
            .param("compareAtMinor", command.compareAtMinor())
            .param("validFrom", timestamp)
            .param("observedAt", timestamp)
            .query((resultSet, rowNum) -> resultSet.getLong("sequence"))
            .single();

        PriceChangeEvent event = new PriceChangeEvent(
            tenantId,
            command.sku(),
            variant.productId(),
            variant.id(),
            priceId,
            currency,
            command.amountMinor(),
            command.compareAtMinor(),
            now.toString(),
            now.toString(),
            eventId,
            sequence
        );
        jdbc.sql("SELECT pg_notify(:channel, :payload)")
            .param("channel", PriceChangeEvent.CHANNEL)
            .param("payload", serialize(event))
            .query((resultSet, rowNum) -> Boolean.TRUE)
            .single();

        return findBySku(tenantId, command.sku());
    }

    @Override
    public PriceChange findPriceChange(PriceChangeEvent event) {
        String tenantId = CatalogRequestIdentity.requireTenantValue(event.tenantId());
        Product product = findBySku(tenantId, event.sku());
        if (product == null) {
            return null;
        }

        Map<String, List<ProductVariant>> variants = findVariantsByProducts(
            tenantId,
            List.of(product.id())
        );
        ProductVariant variant = variants.getOrDefault(product.id(), List.of()).stream()
            .filter(candidate -> candidate.id().equals(event.variantId())
                && candidate.sku().equals(event.sku()))
            .findFirst()
            .orElse(null);
        if (variant == null) {
            return null;
        }
        PriceSnapshot price = new PriceSnapshot(
            event.priceId(),
            event.currency(),
            event.amountMinor(),
            event.compareAtMinor(),
            event.validFrom()
        );
        ProductVariant eventVariant = new ProductVariant(
            variant.id(),
            variant.tenantId(),
            variant.productId(),
            variant.sku(),
            variant.name(),
            variant.options(),
            price
        );
        return new PriceChange(product, eventVariant, price, event.observedAt());
    }

    private String serialize(PriceChangeEvent event) {
        try {
            return objectMapper.writeValueAsString(event);
        } catch (JsonProcessingException error) {
            throw new IllegalStateException("price change notification payload could not be serialized", error);
        }
    }

    private record VariantReference(String id, String productId) {}

    private static Product mapProduct(ResultSet resultSet, int rowNum) throws SQLException {
        return new Product(
            resultSet.getString("product_id"),
            resultSet.getString("tenant_id"),
            resultSet.getString("slug"),
            resultSet.getString("product_sku"),
            resultSet.getString("product_name"),
            resultSet.getString("description"),
            resultSet.getString("brand"),
            new Category(
                resultSet.getString("category_id"),
                resultSet.getString("tenant_id"),
                resultSet.getString("category_slug"),
                resultSet.getString("category_name")
            ),
            mapPrice(resultSet)
        );
    }

    private static ProductVariant mapVariant(ResultSet resultSet, int rowNum) throws SQLException {
        return new ProductVariant(
            resultSet.getString("variant_id"),
            resultSet.getString("tenant_id"),
            resultSet.getString("product_id"),
            resultSet.getString("sku"),
            resultSet.getString("variant_name"),
            resultSet.getString("options"),
            mapPrice(resultSet)
        );
    }

    private static Review mapReview(ResultSet resultSet, int rowNum) throws SQLException {
        return new Review(
            resultSet.getString("id"),
            resultSet.getString("product_id"),
            resultSet.getString("title"),
            resultSet.getString("body"),
            resultSet.getInt("rating"),
            resultSet.getBoolean("verified_purchase"),
            resultSet.getTimestamp("created_at").toInstant().toString()
        );
    }

    private static PriceSnapshot mapPrice(ResultSet resultSet) throws SQLException {
        String id = resultSet.getString("price_id");
        if (id == null) {
            return null;
        }
        Integer compareAt = (Integer) resultSet.getObject("compare_at_minor");
        return new PriceSnapshot(
            id,
            resultSet.getString("price_currency"),
            resultSet.getInt("amount_minor"),
            compareAt,
            resultSet.getTimestamp("price_valid_from").toInstant().toString()
        );
    }

    private static String namedPlaceholders(int count) {
        var placeholders = new StringBuilder();
        for (int index = 0; index < count; index++) {
            if (index > 0) {
                placeholders.append(", ");
            }
            placeholders.append(":productId").append(index);
        }
        return placeholders.toString();
    }
}

package dev.tailrocks.catalog;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.data.redis.core.ScanOptions;
import org.springframework.data.redis.core.StringRedisTemplate;
import org.springframework.stereotype.Component;

import java.time.Duration;
import java.util.Base64;
import java.util.Optional;

interface CatalogCache {
    Optional<Product> getProduct(String tenantId, String sku);

    void putProduct(String tenantId, String sku, Product product);

    Optional<ProductPage> getPage(String key);

    void putPage(String key, ProductPage page);

    void evictProduct(String tenantId, String sku);

    void evictTenant(String tenantId);

    static CatalogCache noop() {
        return new CatalogCache() {
            @Override
            public Optional<Product> getProduct(String tenantId, String sku) {
                return Optional.empty();
            }

            @Override
            public void putProduct(String tenantId, String sku, Product product) {}

            @Override
            public Optional<ProductPage> getPage(String key) {
                return Optional.empty();
            }

            @Override
            public void putPage(String key, ProductPage page) {}

            @Override
            public void evictProduct(String tenantId, String sku) {}

            @Override
            public void evictTenant(String tenantId) {}
        };
    }
}

@Component
class RedisCatalogCache implements CatalogCache {
    private static final Logger LOG = LoggerFactory.getLogger(RedisCatalogCache.class);
    private static final Duration TTL = Duration.ofSeconds(30);
    private static final String PREFIX = "catalog:v2:";

    private final StringRedisTemplate redis;
    private final ObjectMapper objectMapper;

    RedisCatalogCache(StringRedisTemplate redis, ObjectMapper objectMapper) {
        this.redis = redis;
        this.objectMapper = objectMapper;
    }

    @Override
    public Optional<Product> getProduct(String tenantId, String sku) {
        return read(productKey(CatalogRequestIdentity.requireTenantValue(tenantId), sku), Product.class);
    }

    @Override
    public void putProduct(String tenantId, String sku, Product product) {
        write(productKey(CatalogRequestIdentity.requireTenantValue(tenantId), sku), product);
    }

    @Override
    public Optional<ProductPage> getPage(String key) {
        return read(key, ProductPage.class);
    }

    @Override
    public void putPage(String key, ProductPage page) {
        write(key, page);
    }

    @Override
    public void evictProduct(String tenantId, String sku) {
        execute(() -> redis.delete(productKey(CatalogRequestIdentity.requireTenantValue(tenantId), sku)));
    }

    @Override
    public void evictTenant(String tenantId) {
        String scopedTenant = CatalogRequestIdentity.requireTenantValue(tenantId);
        execute(() -> {
            var keys = redis.scan(
                ScanOptions.scanOptions().match(PREFIX + "*:" + encoded(scopedTenant) + ":*").count(100).build()
            );
            try (keys) {
                var deleted = new java.util.ArrayList<String>();
                keys.forEachRemaining(deleted::add);
                if (!deleted.isEmpty()) {
                    redis.delete(deleted);
                }
            }
            return null;
        });
    }

    private <T> Optional<T> read(String key, Class<T> type) {
        try {
            String value = redis.opsForValue().get(key);
            return value == null ? Optional.empty() : Optional.of(objectMapper.readValue(value, type));
        } catch (Exception error) {
            LOG.warn("catalog cache read failed; querying Postgres", error);
            return Optional.empty();
        }
    }

    private void write(String key, Object value) {
        try {
            redis.opsForValue().set(key, objectMapper.writeValueAsString(value), TTL);
        } catch (JsonProcessingException | RuntimeException error) {
            LOG.warn("catalog cache write failed; Postgres remains authoritative", error);
        }
    }

    private void execute(java.util.concurrent.Callable<Object> action) {
        try {
            action.call();
        } catch (Exception error) {
            LOG.warn("catalog cache invalidation failed; stale entries expire after {}", TTL, error);
        }
    }

    static String pageKey(
        String tenantId,
        String search,
        String category,
        String sort,
        int page,
        int size,
        String experience
    ) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        return PREFIX + "page:" + encoded(tenantId) + ":" + encoded(search == null ? "" : search)
            + ":" + encoded(category == null ? "" : category) + ":" + encoded(sort)
            + ":" + page + ":" + size + ":" + encoded(experience);
    }

    private static String productKey(String tenantId, String sku) {
        return PREFIX + "product:" + encoded(tenantId) + ":" + encoded(sku);
    }

    private static String encoded(String value) {
        return Base64.getUrlEncoder().withoutPadding().encodeToString(value.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }
}

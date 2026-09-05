package dev.tailrocks.catalog;

import org.springframework.jdbc.core.simple.JdbcClient;
import org.springframework.stereotype.Repository;

import java.sql.ResultSet;
import java.sql.SQLException;
import java.util.List;

@Repository
class JdbcPriceChangeJournal implements PriceChangeJournal {
    private final JdbcClient jdbc;

    JdbcPriceChangeJournal(JdbcClient jdbc) {
        this.jdbc = jdbc;
    }

    @Override
    public long latestSequence(String tenantId, String sku) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        return jdbc.sql("""
                SELECT COALESCE(MAX(sequence), 0)
                FROM price_change_events
                WHERE tenant_id = :tenantId
                  AND (CAST(:sku AS TEXT) IS NULL OR sku = CAST(:sku AS TEXT))
                """)
            .param("tenantId", tenantId)
            .param("sku", sku)
            .query((resultSet, rowNum) -> resultSet.getLong(1))
            .single();
    }

    @Override
    public List<PriceChangeEvent> eventsAfter(String tenantId, String sku, long sequence) {
        tenantId = CatalogRequestIdentity.requireTenantValue(tenantId);
        return jdbc.sql("""
                SELECT event_id, sequence, tenant_id, sku, product_id, variant_id,
                       price_id, currency, amount_minor, compare_at_minor,
                       valid_from, observed_at
                FROM price_change_events
                WHERE tenant_id = :tenantId
                  AND (CAST(:sku AS TEXT) IS NULL OR sku = CAST(:sku AS TEXT))
                  AND sequence > :sequence
                ORDER BY sequence
                """)
            .param("tenantId", tenantId)
            .param("sku", sku)
            .param("sequence", sequence)
            .query(JdbcPriceChangeJournal::mapEvent)
            .list();
    }

    private static PriceChangeEvent mapEvent(ResultSet resultSet, int rowNum) throws SQLException {
        return new PriceChangeEvent(
            resultSet.getString("tenant_id"),
            resultSet.getString("sku"),
            resultSet.getString("product_id"),
            resultSet.getString("variant_id"),
            resultSet.getString("price_id"),
            resultSet.getString("currency"),
            resultSet.getInt("amount_minor"),
            (Integer) resultSet.getObject("compare_at_minor"),
            resultSet.getTimestamp("valid_from").toInstant().toString(),
            resultSet.getTimestamp("observed_at").toInstant().toString(),
            resultSet.getString("event_id"),
            resultSet.getLong("sequence")
        );
    }
}

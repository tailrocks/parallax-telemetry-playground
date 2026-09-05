package dev.tailrocks.catalog;

import org.postgresql.PGConnection;
import org.postgresql.PGNotification;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.stereotype.Component;
import reactor.core.publisher.Flux;
import reactor.core.publisher.FluxSink;
import reactor.core.scheduler.Schedulers;

import javax.sql.DataSource;
import java.sql.Connection;
import java.sql.Statement;
import java.time.Duration;
import java.util.List;

@Component
final class PostgresPriceChangeListener implements PriceChangeSource {
    private static final Logger LOG = LoggerFactory.getLogger(PostgresPriceChangeListener.class);
    private static final String LISTEN_SQL = "LISTEN " + PriceChangeEvent.CHANNEL;
    private static final int NOTIFICATION_WAIT_MILLIS = 1000;
    private static final Duration RECONNECT_DELAY = Duration.ofMillis(250);

    private final DataSource dataSource;
    private final PriceChangeJournal journal;
    private final Duration reconnectDelay;

    @Autowired
    PostgresPriceChangeListener(DataSource dataSource, PriceChangeJournal journal) {
        this(dataSource, journal, RECONNECT_DELAY);
    }

    PostgresPriceChangeListener(
        DataSource dataSource,
        PriceChangeJournal journal,
        Duration reconnectDelay
    ) {
        this.dataSource = dataSource;
        this.journal = journal;
        this.reconnectDelay = reconnectDelay;
    }

    @Override
    public Flux<PriceChangeEvent> stream(String tenantId, String sku) {
        String scopedTenant = CatalogRequestIdentity.requireTenantValue(tenantId);
        return Flux.<PriceChangeEvent>create(sink -> consume(sink, scopedTenant, sku))
            .subscribeOn(Schedulers.boundedElastic());
    }

    private void consume(FluxSink<PriceChangeEvent> sink, String tenantId, String sku) {
        long sequence = 0;
        boolean baselineCaptured = false;
        while (!sink.isCancelled()) {
            try {
                if (!baselineCaptured) {
                    sequence = journal.latestSequence(tenantId, sku);
                    baselineCaptured = true;
                }
                try (Connection connection = dataSource.getConnection()) {
                    connection.setAutoCommit(true);
                    PGConnection pgConnection = connection.unwrap(PGConnection.class);
                    try (Statement statement = connection.createStatement()) {
                        statement.execute(LISTEN_SQL);
                    }

                    sequence = emitAfter(sink, tenantId, sku, sequence);
                    while (!sink.isCancelled()) {
                        PGNotification[] notifications =
                            pgConnection.getNotifications(NOTIFICATION_WAIT_MILLIS);
                        if (hasPriceChangeNotification(notifications)) {
                            // NOTIFY is only a wake-up. The table is the canonical,
                            // replayable source and closes every disconnect window.
                            sequence = emitAfter(sink, tenantId, sku, sequence);
                        }
                    }
                }
            } catch (Exception error) {
                if (sink.isCancelled()) {
                    return;
                }
                LOG.warn("price change LISTEN connection failed; reconnecting", error);
                try {
                    Thread.sleep(reconnectDelay.toMillis());
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    if (!sink.isCancelled()) {
                        sink.error(interrupted);
                    }
                    return;
                }
            }
        }
    }

    private long emitAfter(FluxSink<PriceChangeEvent> sink, String tenantId, String sku, long sequence) {
        List<PriceChangeEvent> events = journal.eventsAfter(tenantId, sku, sequence);
        for (PriceChangeEvent event : events) {
            if (sink.isCancelled()) {
                return sequence;
            }
            if (event.sequence() <= sequence) {
                continue;
            }
            sink.next(event);
            sequence = event.sequence();
        }
        return sequence;
    }

    private static boolean hasPriceChangeNotification(PGNotification[] notifications) {
        if (notifications == null) {
            return false;
        }
        for (PGNotification notification : notifications) {
            if (PriceChangeEvent.CHANNEL.equals(notification.getName())) {
                return true;
            }
        }
        return false;
    }
}

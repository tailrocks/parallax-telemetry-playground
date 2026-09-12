package dev.tailrocks.catalog;

import org.junit.jupiter.api.Test;
import org.postgresql.PGConnection;
import org.postgresql.PGNotification;
import reactor.core.publisher.Flux;

import javax.sql.DataSource;
import java.sql.Connection;
import java.sql.SQLException;
import java.sql.Statement;
import java.time.Duration;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.mockito.ArgumentMatchers.anyInt;
import static org.mockito.Mockito.atLeast;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

class PostgresPriceChangeListenerTest {
    @Test
    void reconnects_and_replays_events_committed_while_listen_connection_was_down() throws Exception {
        DataSource dataSource = mock(DataSource.class);
        Connection firstConnection = mock(Connection.class);
        Connection secondConnection = mock(Connection.class);
        Statement firstStatement = mock(Statement.class);
        Statement secondStatement = mock(Statement.class);
        PGConnection firstPgConnection = mock(PGConnection.class);
        PGConnection secondPgConnection = mock(PGConnection.class);
        PriceChangeJournal journal = mock(PriceChangeJournal.class);
        PriceChangeEvent event = event(11);

        when(dataSource.getConnection()).thenReturn(firstConnection, secondConnection);
        when(firstConnection.createStatement()).thenReturn(firstStatement);
        when(secondConnection.createStatement()).thenReturn(secondStatement);
        when(firstConnection.unwrap(PGConnection.class)).thenReturn(firstPgConnection);
        when(secondConnection.unwrap(PGConnection.class)).thenReturn(secondPgConnection);
        when(firstPgConnection.getNotifications(anyInt())).thenThrow(new SQLException("socket reset"));
        when(journal.latestSequence("tenant-acme", "WIDGET-1")).thenReturn(10L);
        when(journal.eventsAfter("tenant-acme", "WIDGET-1", 10L))
            .thenReturn(List.of())
            .thenReturn(List.of(event));

        PostgresPriceChangeListener listener = new PostgresPriceChangeListener(
            dataSource,
            journal,
            Duration.ofMillis(1)
        );

        PriceChangeEvent replayed = Flux.from(listener.stream("tenant-acme", "WIDGET-1"))
            .take(1)
            .blockFirst(Duration.ofSeconds(3));

        assertNotNull(replayed);
        assertEquals(event.eventId(), replayed.eventId());
        assertEquals(11, replayed.sequence());
        verify(dataSource, atLeast(2)).getConnection();
        verify(journal, atLeast(2)).eventsAfter("tenant-acme", "WIDGET-1", 10L);
    }

    @Test
    void uses_notification_only_as_a_wakeup_for_the_durable_journal() throws Exception {
        DataSource dataSource = mock(DataSource.class);
        Connection connection = mock(Connection.class);
        Statement statement = mock(Statement.class);
        PGConnection pgConnection = mock(PGConnection.class);
        PGNotification notification = mock(PGNotification.class);
        PriceChangeJournal journal = mock(PriceChangeJournal.class);
        PriceChangeEvent event = event(2);

        when(dataSource.getConnection()).thenReturn(connection);
        when(connection.createStatement()).thenReturn(statement);
        when(connection.unwrap(PGConnection.class)).thenReturn(pgConnection);
        when(journal.latestSequence("tenant-acme", null)).thenReturn(1L);
        when(journal.eventsAfter("tenant-acme", null, 1L))
            .thenReturn(List.of())
            .thenReturn(List.of(event));
        when(notification.getName()).thenReturn(PriceChangeEvent.CHANNEL);
        when(pgConnection.getNotifications(anyInt()))
            .thenReturn(new PGNotification[] {notification});

        PostgresPriceChangeListener listener = new PostgresPriceChangeListener(
            dataSource,
            journal,
            Duration.ofMillis(1)
        );

        PriceChangeEvent replayed = listener.stream("tenant-acme", null)
            .take(1)
            .blockFirst(Duration.ofSeconds(3));

        assertNotNull(replayed);
        assertEquals(event.priceId(), replayed.priceId());
        verify(journal, atLeast(2)).eventsAfter("tenant-acme", null, 1L);
    }

    private static PriceChangeEvent event(long sequence) {
        return new PriceChangeEvent(
            "tenant-acme", "WIDGET-1", "prod-acme-widget", "var-acme-widget-1",
            "price-" + sequence, "USD", 2199, 2499,
            "2026-09-04T00:00:00Z", "2026-09-04T00:00:00Z",
            "event-" + sequence, sequence
        );
    }
}

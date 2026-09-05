package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.atLeastOnce;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.fasterxml.jackson.databind.ObjectMapper;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.jdbc.core.RowMapper;
import org.springframework.transaction.support.TransactionCallback;
import org.springframework.transaction.support.TransactionTemplate;

class FulfillmentRepositoryTest {
    @Test
    void validates_payment_before_no_op_for_a_terminal_order() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(new ClaimRow(
                    "processing",
                    1,
                    "lease-terminal",
                    Instant.now().plusSeconds(30)
                )),
                List.of(new OrderSnapshot(
                    "order-lease-1",
                    "tenant-acme",
                    "customer-acme-ava",
                    "USD",
                    9177,
                    "cancelled",
                    "captured",
                    Instant.now()
                )),
                List.of(new PaymentSnapshot(
                    "payment-terminal",
                    "tenant-acme",
                    "order-lease-1",
                    "captured",
                    9177,
                    "USD"
                ))
            );
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        ProcessingResult result = repository.processOrder(event());

        assertTrue(result.noOp());
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        ArgumentCaptor<Object[]> arguments = ArgumentCaptor.forClass(Object[].class);
        verify(jdbc, times(3)).query(sql.capture(), any(RowMapper.class), arguments.capture());
        assertTrue(sql.getAllValues().get(1).contains("FOR UPDATE OF o"));
        assertTrue(sql.getAllValues().get(2).contains("SELECT id, tenant_id, order_id, status"));
        assertEquals("tenant-acme", arguments.getAllValues().get(2)[0]);
        assertEquals("order-lease-1", arguments.getAllValues().get(2)[1]);
        verify(jdbc, org.mockito.Mockito.never()).update(anyString(), any(Object[].class));
    }

    @Test
    void rejects_a_terminal_payment_event_with_mismatched_payment_identity() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        CommerceEvent event = paymentEvent(
            "payment.captured",
            "captured",
            "payment-terminal",
            "order-terminal-payment"
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(new ClaimRow(
                    "processing",
                    1,
                    "lease-terminal-payment",
                    Instant.now().plusSeconds(30)
                )),
                List.of(new OrderSnapshot(
                    "order-terminal-payment",
                    "tenant-acme",
                    "customer-acme-ava",
                    "USD",
                    9177,
                    "shipped",
                    "captured",
                    Instant.now()
                )),
                List.of(new PaymentSnapshot(
                    "payment-other",
                    "tenant-acme",
                    "order-terminal-payment",
                    "captured",
                    9177,
                    "USD"
                ))
            );
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        assertThrows(PermanentEventException.class, () -> repository.processOrder(event));

        verify(jdbc).update(
            org.mockito.ArgumentMatchers.contains("UPDATE fulfillment_processed_events"),
            any(Object[].class)
        );
        verify(jdbc, never()).update(
            org.mockito.ArgumentMatchers.contains("INSERT INTO shipments"),
            any(Object[].class)
        );
    }

    @Test
    void does_not_cancel_a_shipped_order_or_its_terminal_fulfillment() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(new ClaimRow(
                    "processing",
                    1,
                    "lease-cancel-terminal",
                    Instant.now().plusSeconds(30)
                )),
                List.of(new OrderSnapshot(
                    "order-shipped",
                    "tenant-acme",
                    "customer-acme-ava",
                    "USD",
                    9177,
                    "shipped",
                    "captured",
                    Instant.now()
                ))
            );
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        ProcessingResult result = repository.processOrder(cancellationEvent("order-shipped"));

        assertTrue(result.noOp());
        verify(jdbc, never()).update(anyString(), any(Object[].class));
    }

    @Test
    void cancellation_uses_conditional_transitions_for_order_and_shipment() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(new ClaimRow(
                    "processing",
                    1,
                    "lease-cancel",
                    Instant.now().plusSeconds(30)
                )),
                List.of(new OrderSnapshot(
                    "order-cancel",
                    "tenant-acme",
                    "customer-acme-ava",
                    "USD",
                    9177,
                    "paid",
                    "captured",
                    Instant.now()
                ))
            );
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        ProcessingResult result = repository.processOrder(cancellationEvent("order-cancel"));

        assertTrue(result.cancellation());
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        verify(jdbc, times(5)).update(sql.capture(), any(Object[].class));
        assertTrue(sql.getAllValues().stream().anyMatch(statement ->
            statement.contains("status IN ('pending', 'confirmed', 'paid', 'processing')")
        ));
        assertTrue(sql.getAllValues().stream().anyMatch(statement ->
            statement.contains("UPDATE fulfillment_effects")
                && statement.contains("status = 'cancelled'")
        ));
        assertTrue(sql.getAllValues().stream().anyMatch(statement ->
            statement.contains("status IN ('pending', 'label_created')")
        ));
    }

    @Test
    void persists_an_explicit_event_session_id_in_postgres_analytics() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        CommerceEvent event = CommerceEvent.parse(new ObjectMapper(), """
            {
              "event_id":"event-session-2",
              "event_key":"order.paid:order-session-2",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.paid",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-session-2",
              "session_id":"session-explicit",
              "payload":{"order_id":"order-session-2"}
            }
            """.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(List.of(new ClaimRow(
                "processing",
                1,
                "lease-session-2",
                Instant.now().plusSeconds(30)
            )));
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        AnalyticsClaim claim = repository.claimAnalytics(event);

        assertEquals("lease-session-2", claim.leaseToken());
        ArgumentCaptor<Object[]> arguments = ArgumentCaptor.forClass(Object[].class);
        verify(jdbc).update(
            org.mockito.ArgumentMatchers.contains("INSERT INTO analytics_events"),
            arguments.capture()
        );
        assertEquals("session-explicit", arguments.getValue()[4]);
    }

    @Test
    void claim_sql_reclaims_only_an_expired_processing_lease_with_a_new_token() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(List.of(new ClaimRow(
                "processing",
                2,
                "new-lease-token",
                Instant.now().plusSeconds(30)
            )));

        ClaimDecision decision = repository.acquireClaim(
            FulfillmentRepository.ORDER_CONSUMER,
            event()
        );

        assertEquals(ClaimState.ACQUIRED, decision.state());
        assertEquals("new-lease-token", decision.leaseToken());
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        ArgumentCaptor<Object[]> arguments = ArgumentCaptor.forClass(Object[].class);
        verify(jdbc).query(sql.capture(), any(RowMapper.class), arguments.capture());
        assertTrue(sql.getValue().contains("lease_until <= CURRENT_TIMESTAMP"));
        assertTrue(sql.getValue().contains("lease_token = EXCLUDED.lease_token"));
        assertEquals(12, arguments.getValue().length);
        assertEquals("tenant-acme", arguments.getValue()[0]);
        assertEquals(FulfillmentRepository.ORDER_CONSUMER, arguments.getValue()[1]);
        assertEquals("fulfillment:tenant-acme:order-lease-1", arguments.getValue()[2]);
        assertEquals("tenant-acme", arguments.getValue()[4]);
        assertEquals(FulfillmentRepository.ORDER_CONSUMER, arguments.getValue()[5]);
        assertEquals("fulfillment:tenant-acme:order-lease-1", arguments.getValue()[6]);
        assertEquals("order.created", arguments.getValue()[7]);
        assertEquals("order-lease-1", arguments.getValue()[8]);
        assertEquals(30L, arguments.getValue()[10]);
        assertEquals(FulfillmentRepository.MAX_CLAIM_ATTEMPTS, arguments.getValue()[11]);
    }

    @Test
    void effect_claim_is_fenced_by_order_version_status_and_current_event_lease() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(List.of());
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        assertTrue(repository.claimEffects(event(), "current-lease").isEmpty());

        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        verify(jdbc).query(sql.capture(), any(RowMapper.class), any(Object[].class));
        assertTrue(sql.getValue().contains("order_row.updated_at = effect.aggregate_version"));
        assertTrue(sql.getValue().contains("order_row.status IN ('pending', 'confirmed', 'paid', 'processing')"));
        assertTrue(sql.getValue().contains("claim.lease_token = ?"));
        assertTrue(sql.getValue().contains("effect.operation = ?"));
    }

    @Test
    void payment_failure_loads_the_exact_tenant_payment_and_proves_its_order() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        CommerceEvent event = paymentEvent("payment.failed", "failed", "payment-exact", "order-exact");
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(new ClaimRow("processing", 1, "lease-payment", Instant.now().plusSeconds(30))),
                List.of(new OrderSnapshot(
                    "order-exact", "tenant-acme", "customer-acme-ava", "USD", 9177,
                    "paid", "failed", Instant.now()
                )),
                List.of(new PaymentSnapshot(
                    "payment-exact", "tenant-acme", "order-exact", "failed", 9177, "USD"
                ))
            );
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        ProcessingResult result = repository.processOrder(event);

        assertTrue(result.cancellation());
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        ArgumentCaptor<Object[]> arguments = ArgumentCaptor.forClass(Object[].class);
        verify(jdbc, times(3)).query(sql.capture(), any(RowMapper.class), arguments.capture());
        assertTrue(sql.getAllValues().get(2).contains("SELECT id, tenant_id, order_id, status"));
        assertTrue(sql.getAllValues().get(2).contains("WHERE tenant_id = ? AND id = ?"));
        assertEquals("tenant-acme", arguments.getAllValues().get(2)[0]);
        assertEquals("payment-exact", arguments.getAllValues().get(2)[1]);
    }

    @Test
    void validates_before_dispatch_and_finalizes_after_dispatch_in_separate_transactions() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        FulfillmentEffect effect = new FulfillmentEffect(
            "tenant-acme", "order-fenced", "fulfillment:tenant-acme:order-fenced",
            "fulfillment", "event", "{}", null, "effect-lease"
        );
        List<String> phases = new ArrayList<>();
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(List.of(true));
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            phases.add("transaction-start");
            try {
                return callback.doInTransaction(null);
            } finally {
                phases.add("transaction-end");
            }
        });
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);

        repository.validateEffectClaim(
            effect,
            "fulfillment:tenant-acme:order-fenced",
            "claim-lease"
        );

        Runnable externalDispatch = () -> {
            phases.add("external-dispatch");
        };
        externalDispatch.run();
        repository.publishEffect(effect, "fulfillment:tenant-acme:order-fenced", "claim-lease");

        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        verify(jdbc).query(sql.capture(), any(RowMapper.class), any(Object[].class));
        assertTrue(sql.getValue().contains("WITH locked_order AS MATERIALIZED"));
        assertTrue(sql.getValue().contains("FOR UPDATE OF effect"));
        assertTrue(sql.getValue().contains("effect.aggregate_version = order_row.updated_at"));
        assertTrue(sql.getValue().contains("effect.claim_token = ?"));
        assertTrue(sql.getValue().contains("claim.lease_token = ?"));
        verify(jdbc).update(org.mockito.ArgumentMatchers.contains("SET status = 'published'"), any(Object[].class));
        assertEquals(
            List.of(
                "transaction-start", "transaction-end", "external-dispatch",
                "transaction-start", "transaction-end"
            ),
            phases
        );
    }

    @Test
    void validation_fence_rejects_an_effect_superseded_by_cancellation() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        FulfillmentEffect effect = new FulfillmentEffect(
            "tenant-acme", "order-cancelled", "fulfillment:tenant-acme:order-cancelled",
            "fulfillment", "event", "{}", null, "effect-lease"
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(List.of());
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        assertThrows(
            RetryableEventException.class,
            () -> repository.validateEffectClaim(
                effect,
                "fulfillment:tenant-acme:order-cancelled",
                "claim-lease"
            )
        );

        verify(jdbc, never()).update(anyString(), any(Object[].class));
    }

    @Test
    void finalization_fence_rejects_a_claim_lost_after_external_dispatch() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        FulfillmentEffect effect = new FulfillmentEffect(
            "tenant-acme", "order-lost", "fulfillment:tenant-acme:order-lost",
            "fulfillment", "event", "{}", null, "effect-lease"
        );
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(0);

        assertThrows(
            RetryableEventException.class,
            () -> repository.publishEffect(
                effect,
                "fulfillment:tenant-acme:order-lost",
                "claim-lease"
            )
        );

        verify(jdbc).update(
            org.mockito.ArgumentMatchers.contains("SET status = 'published'"),
            any(Object[].class)
        );
    }

    @Test
    void active_claim_is_not_reported_as_complete_and_can_be_deferred() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(),
                List.of(new ClaimRow(
                    "processing",
                    2,
                    "existing-lease-token",
                    Instant.now().plusSeconds(30)
                ))
            );

        ClaimDecision decision = repository.acquireClaim(
            FulfillmentRepository.ORDER_CONSUMER,
            event()
        );

        assertEquals(ClaimState.ACTIVE, decision.state());
        assertEquals(null, decision.leaseToken());
    }

    @Test
    void exhausted_expired_order_claim_is_terminalized_before_returning_dead_lettered() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(),
                List.of(new ClaimRow("dead_lettered", FulfillmentRepository.MAX_CLAIM_ATTEMPTS, null, null))
            );

        ClaimDecision decision = repository.acquireClaim(
            FulfillmentRepository.ORDER_CONSUMER,
            event()
        );

        assertEquals(ClaimState.DEAD_LETTERED, decision.state());
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        verify(jdbc, times(2)).query(sql.capture(), any(RowMapper.class), any(Object[].class));
        assertTrue(sql.getAllValues().get(0).contains("WITH exhausted AS"));
        assertTrue(sql.getAllValues().get(0).contains("status = 'dead_lettered'"));
        assertTrue(sql.getAllValues().get(0).contains("dead_lettered_at"));
        assertTrue(sql.getAllValues().get(0).contains("lease_until <= CURRENT_TIMESTAMP"));
    }

    @Test
    void exhausted_expired_analytics_claim_is_terminalized_before_returning_dead_lettered() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        TransactionTemplate transactions = mock(TransactionTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            transactions,
            new ObjectMapper()
        );
        when(jdbc.query(anyString(), any(RowMapper.class), any(Object[].class)))
            .thenReturn(
                List.of(),
                List.of(new ClaimRow("dead_lettered", FulfillmentRepository.MAX_CLAIM_ATTEMPTS, null, null))
            );
        when(transactions.execute(any())).thenAnswer(invocation -> {
            @SuppressWarnings("unchecked")
            TransactionCallback<Object> callback = invocation.getArgument(0);
            return callback.doInTransaction(null);
        });

        AnalyticsClaim claim = repository.claimAnalytics(event());

        assertTrue(claim.deadLettered());
    }

    @Test
    void completion_requires_the_current_unexpired_lease() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper()
        );
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(0);

        assertThrows(
            RetryableEventException.class,
            () -> repository.markCompleted(
                event().tenantId(),
                FulfillmentRepository.ORDER_CONSUMER,
                event().eventKey(),
                "lease-token"
            )
        );
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        verify(jdbc).update(sql.capture(), any(Object[].class));
        assertTrue(sql.getValue().contains("lease_token = ?"));
        assertTrue(sql.getValue().contains("lease_until > CURRENT_TIMESTAMP"));
    }

    @Test
    void permanent_failure_dead_letters_immediately_with_timestamp() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper()
        );
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);

        repository.markFailed(
            FulfillmentRepository.ORDER_CONSUMER,
            event().tenantId(),
            event().eventKey(),
            "lease-token",
            new PermanentEventException("invalid effect")
        );
        ArgumentCaptor<String> sql = ArgumentCaptor.forClass(String.class);
        ArgumentCaptor<Object[]> arguments = ArgumentCaptor.forClass(Object[].class);
        verify(jdbc).update(sql.capture(), arguments.capture());
        assertTrue(sql.getValue().contains("WHEN ? THEN 'dead_lettered'"));
        assertTrue(sql.getValue().contains("WHEN ? THEN CURRENT_TIMESTAMP"));
        assertTrue(sql.getValue().contains("status = 'processing'"));
        assertTrue(sql.getValue().contains("lease_token = ?"));
        assertTrue(sql.getValue().contains("(? OR lease_until > CURRENT_TIMESTAMP)"));
        assertEquals(Boolean.TRUE, arguments.getValue()[0]);
        assertEquals(Boolean.TRUE, arguments.getValue()[3]);
        assertEquals(Boolean.TRUE, arguments.getValue()[9]);
    }

    @Test
    void heartbeat_renews_on_a_bounded_schedule_and_stops_after_processing() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        ScheduledExecutorService scheduler = mock(ScheduledExecutorService.class);
        ScheduledFuture<?> future = mock(ScheduledFuture.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper(),
            scheduler
        );
        ArgumentCaptor<Runnable> renewal = ArgumentCaptor.forClass(Runnable.class);
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(1);
        when(scheduler.scheduleAtFixedRate(
            renewal.capture(),
            eq(10_000L),
            eq(10_000L),
            eq(TimeUnit.MILLISECONDS)
        )).thenAnswer(invocation -> future);

        try (LeaseHeartbeat heartbeat = repository.maintainLease(
            event().tenantId(),
            FulfillmentRepository.ORDER_CONSUMER,
            "order-lease-heartbeat",
            "lease-heartbeat"
        )) {
            renewal.getValue().run();
            heartbeat.verify();
        }

        verify(jdbc, atLeastOnce()).update(
            org.mockito.ArgumentMatchers.contains("SET lease_until"),
            any(Object[].class)
        );
        verify(future).cancel(false);
    }

    @Test
    void heartbeat_failure_fences_side_effect_completion() {
        JdbcTemplate jdbc = mock(JdbcTemplate.class);
        ScheduledExecutorService scheduler = mock(ScheduledExecutorService.class);
        ScheduledFuture<?> future = mock(ScheduledFuture.class);
        FulfillmentRepository repository = new FulfillmentRepository(
            jdbc,
            mock(TransactionTemplate.class),
            new ObjectMapper(),
            scheduler
        );
        ArgumentCaptor<Runnable> renewal = ArgumentCaptor.forClass(Runnable.class);
        when(jdbc.update(anyString(), any(Object[].class))).thenReturn(0);
        when(scheduler.scheduleAtFixedRate(
            renewal.capture(),
            eq(10_000L),
            eq(10_000L),
            eq(TimeUnit.MILLISECONDS)
        )).thenAnswer(invocation -> future);

        LeaseHeartbeat heartbeat = repository.maintainLease(
            event().tenantId(),
            FulfillmentRepository.ORDER_CONSUMER,
            "order-lease-lost",
            "lease-lost"
        );
        renewal.getValue().run();

        assertThrows(RetryableEventException.class, heartbeat::verify);
        verify(future).cancel(false);
        assertThrows(RetryableEventException.class, heartbeat::close);
    }

    @Test
    void refuses_processing_without_a_lease_heartbeat_executor() {
        FulfillmentRepository repository = new FulfillmentRepository(
            mock(JdbcTemplate.class),
            mock(TransactionTemplate.class),
            new ObjectMapper()
        );

        assertThrows(
            IllegalStateException.class,
            () -> repository.maintainLease(
                event().tenantId(),
                FulfillmentRepository.ORDER_CONSUMER,
                "order-without-heartbeat",
                "lease-without-heartbeat"
            )
        );
    }

    private static CommerceEvent event() {
        return CommerceEvent.parse(new ObjectMapper(), """
            {
              "event_id":"event-lease-1",
              "event_key":"order.created:order-lease-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.created",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-lease-1",
              "payload":{
                "order_id":"order-lease-1",
                "currency":"USD",
                "total_minor":9177,
                "payment_status":"captured"
              }
            }
            """.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }

    private static CommerceEvent paymentEvent(
        String eventType,
        String paymentStatus,
        String paymentId,
        String orderId
    ) {
        return CommerceEvent.parse(new ObjectMapper(), ("""
            {
              "event_id":"event-%s",
              "event_key":"%s:%s",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"%s",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"payment",
              "aggregate_id":"%s",
              "payload":{
                "order_id":"%s",
                "payment_id":"%s",
                "amount_minor":9177,
                "currency":"USD",
                "payment_status":"%s"
              }
            }
            """).formatted(
                eventType,
                eventType,
                paymentId,
                eventType,
                paymentId,
                orderId,
                paymentId,
                paymentStatus
            ).getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }

    private static CommerceEvent cancellationEvent(String orderId) {
        return CommerceEvent.parse(new ObjectMapper(), ("""
            {
              "event_id":"event-cancel-%s",
              "event_key":"order.cancelled:%s",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.cancelled",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"%s",
              "payload":{"order_id":"%s"}
            }
            """).formatted(orderId, orderId, orderId, orderId)
            .getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }
}

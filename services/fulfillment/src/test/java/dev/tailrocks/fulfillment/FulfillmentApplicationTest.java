package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import org.junit.jupiter.api.Test;

class FulfillmentApplicationTest {
    @Test
    void reserves_a_heartbeat_worker_for_each_possible_claim() {
        FulfillmentApplication application = new FulfillmentApplication();
        ScheduledExecutorService executor = application.fulfillmentLeaseExecutor(4, 8);

        assertEquals(8, ((ScheduledThreadPoolExecutor) executor).getCorePoolSize());
        executor.shutdownNow();
    }

    @Test
    void rejects_a_heartbeat_pool_that_can_starve_claims() {
        FulfillmentApplication application = new FulfillmentApplication();

        assertThrows(
            IllegalStateException.class,
            () -> application.fulfillmentLeaseExecutor(4, 7)
        );
    }
}

package dev.tailrocks.catalog;

import java.util.List;

interface PriceChangeJournal {
    long latestSequence(String tenantId, String sku);

    List<PriceChangeEvent> eventsAfter(String tenantId, String sku, long sequence);
}

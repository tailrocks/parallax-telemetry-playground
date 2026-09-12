package dev.tailrocks.catalog;

import reactor.core.publisher.Flux;

interface PriceChangeSource {
    Flux<PriceChangeEvent> stream(String tenantId, String sku);
}

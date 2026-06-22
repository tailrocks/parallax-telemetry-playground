package dev.tailrocks.catalog;

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.BatchMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.stereotype.Controller;
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Client;

import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

@SpringBootApplication
public class CatalogApplication {
    public static void main(String[] args) {
        SpringApplication.run(CatalogApplication.class, args);
    }
}

record Product(String id, String sku, String name, int priceMinor) {}

record Review(String text, int stars) {}

@Controller
class ProductController {
    private static final List<Product> CATALOG = List.of(
        new Product("1", "WIDGET-1", "Widget", 1999),
        new Product("2", "GADGET-1", "Gadget", 4999)
    );

    @QueryMapping
    Product product(@Argument String sku) {
        return CATALOG.stream().filter(p -> p.sku().equals(sku)).findFirst().orElse(null);
    }

    // A14: OpenFeature flag evaluation (flagd provider) — the evaluation is
    // surfaced as feature_flag.* telemetry by the OTel hook at runtime.
    private final Client flags = OpenFeatureAPI.getInstance().getClient();

    @QueryMapping
    List<Product> products() {
        boolean promo = flags.getBooleanValue("catalogPromo", false);
        return promo ? CATALOG : CATALOG;
    }

    // A6: per-product `reviews` resolved via a @BatchMapping — Spring GraphQL
    // batches all products' review fetches into ONE DataLoader call, so the
    // trace shows a single batched fetch instead of an N+1 fan of per-product
    // calls. Contrast with a plain @SchemaMapping (which would be N+1).
    @BatchMapping
    Map<Product, List<Review>> reviews(List<Product> products) {
        return products.stream().collect(Collectors.toMap(
            p -> p,
            p -> List.of(new Review("solid " + p.name(), 5), new Review("ok", 3))
        ));
    }
}

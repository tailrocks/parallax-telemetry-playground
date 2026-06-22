package dev.tailrocks.catalog;

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.stereotype.Controller;

import java.util.List;

@SpringBootApplication
public class CatalogApplication {
    public static void main(String[] args) {
        SpringApplication.run(CatalogApplication.class, args);
    }
}

record Product(String id, String sku, String name, int priceMinor) {}

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

    @QueryMapping
    List<Product> products() { return CATALOG; }
}

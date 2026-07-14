package dev.tailrocks.catalog;

import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import java.util.List;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.micrometer.tracing.test.autoconfigure.AutoConfigureTracing;
import org.springframework.boot.test.context.TestConfiguration;
import org.springframework.boot.graphql.test.autoconfigure.GraphQlTest;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Import;
import org.springframework.graphql.test.tester.GraphQlTester;
import org.springframework.test.context.bean.override.mockito.MockitoBean;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.Mockito.when;

@GraphQlTest(controllers = ProductController.class)
@AutoConfigureTracing
@Import(CatalogGraphQlSliceTest.Meters.class)
class CatalogGraphQlSliceTest {
    @Autowired
    private GraphQlTester graphQlTester;

    @MockitoBean
    private CatalogRepository catalog;

    @BeforeEach
    void products() {
        when(catalog.findAll()).thenReturn(List.of(
            new Product("1", "WIDGET-1", "Widget", 1999),
            new Product("2", "GADGET-1", "Gadget", 4999)
        ));
        when(catalog.findBySku("WIDGET-1")).thenReturn(
            new Product("1", "WIDGET-1", "Widget", 1999)
        );
    }

    @Test
    void serves_batched_and_n_plus_one_fields_from_the_graphql_slice() {
        GraphQlTester.Response response = graphQlTester.document("""
                { products { sku reviews { stars } reviewsSlow { text } } }
                """)
            .execute();
        response.path("products[0].sku").entity(String.class).isEqualTo("WIDGET-1");
        response.path("products[0].reviews[0].stars").entity(Integer.class).isEqualTo(5);
        response.path("products[0].reviewsSlow[0].text").entity(String.class).satisfies(
                text -> assertEquals("solid Widget", text)
        );
    }

    @Test
    void exposes_the_deterministic_partial_error_shape() {
        graphQlTester.document("{ products { sku riskScore } }")
            .execute()
            .errors()
            .satisfy(errors -> assertEquals(1, errors.size()));
    }

    @TestConfiguration(proxyBeanMethods = false)
    static class Meters {
        @Bean
        MeterRegistry meterRegistry() {
            return new SimpleMeterRegistry();
        }
    }
}

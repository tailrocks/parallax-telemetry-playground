package dev.tailrocks.catalog;

import org.junit.jupiter.api.Test;
import org.springframework.http.HttpHeaders;
import org.springframework.graphql.server.WebGraphQlRequest;
import org.springframework.graphql.server.WebGraphQlResponse;
import org.springframework.util.LinkedMultiValueMap;
import reactor.core.publisher.Mono;

import java.net.URI;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

class CatalogGraphQlSecurityInterceptorTest {
    private final CatalogGraphQlSecurityInterceptor interceptor =
        new CatalogGraphQlSecurityInterceptor("catalog-admin:tenant-acme:research-secret");

    @Test
    void derives_the_admin_tenant_only_from_a_matching_bearer_token() {
        CatalogAdminIdentity identity = interceptor.authenticate(
            "Bearer catalog-admin:tenant-acme:research-secret"
        );

        assertEquals("tenant-acme", identity.tenantId());
    }

    @Test
    void rejects_a_token_that_cannot_authorize_the_configured_admin_identity() {
        assertNull(interceptor.authenticate("Bearer catalog-admin:tenant-other:research-secret"));
        assertNull(interceptor.authenticate("Bearer catalog-admin:tenant-acme:wrong-secret"));
        assertNull(interceptor.authenticate(null));
    }

    @Test
    void recognizes_no_store_as_a_cache_control_directive() {
        HttpHeaders headers = new HttpHeaders();
        headers.add(HttpHeaders.CACHE_CONTROL, "max-age=0, NO-STORE");

        assertTrue(CatalogGraphQlSecurityInterceptor.hasNoStore(headers));
        assertFalse(CatalogGraphQlSecurityInterceptor.hasNoStore(
            HttpHeaders.readOnlyHttpHeaders(new HttpHeaders())
        ));
    }

    @Test
    void marks_the_graphql_context_for_a_no_store_request() {
        HttpHeaders headers = new HttpHeaders();
        headers.set(HttpHeaders.CACHE_CONTROL, "no-store");
        WebGraphQlRequest request = new WebGraphQlRequest(
            URI.create("http://localhost/graphql"),
            headers,
            new LinkedMultiValueMap<>(),
            Map.of(),
            Map.of("query", "{ __typename }"),
            "test-request",
            Locale.ROOT
        );
        AtomicReference<graphql.GraphQLContext> executionContext = new AtomicReference<>();

        interceptor.intercept(request, ignored -> {
            executionContext.set(request.toExecutionInput().getGraphQLContext());
            return Mono.<WebGraphQlResponse>empty();
        }).block();

        assertTrue(CatalogGraphQlSecurityInterceptor.isNoStore(executionContext.get()));
    }

    @Test
    void refuses_a_malformed_configured_token_instead_of_creating_an_identity() {
        CatalogGraphQlSecurityInterceptor malformed =
            new CatalogGraphQlSecurityInterceptor("tenant-acme");

        assertNull(malformed.authenticate("Bearer tenant-acme"));
    }

    @Test
    void resolves_a_consistent_header_and_w3c_baggage_identity() {
        HttpHeaders headers = new HttpHeaders();
        headers.add("x-tenant-id", "tenant-acme");
        headers.add("baggage", "tenant.id=tenant-acme");

        CatalogRequestIdentity identity = CatalogRequestIdentity.fromHeaders(headers);

        assertEquals("tenant-acme", identity.tenantId());
        assertEquals("tenant-acme", CatalogRequestIdentity.resolve(
            graphql.GraphQLContext.of(Map.of(CatalogRequestIdentity.CONTEXT_KEY, identity)),
            null
        ));
    }

    @Test
    void rejects_missing_and_conflicting_tenant_identity() {
        assertThrows(IllegalArgumentException.class, () -> CatalogRequestIdentity.resolve(
            graphql.GraphQLContext.getDefault(), null
        ));

        HttpHeaders headers = new HttpHeaders();
        headers.add("x-tenant-id", "tenant-acme");
        headers.add("baggage", "tenant.id=tenant-other");
        CatalogRequestIdentity identity = CatalogRequestIdentity.fromHeaders(headers);
        assertThrows(IllegalArgumentException.class, () -> CatalogRequestIdentity.resolve(
            graphql.GraphQLContext.of(Map.of(CatalogRequestIdentity.CONTEXT_KEY, identity)),
            null
        ));
    }
}

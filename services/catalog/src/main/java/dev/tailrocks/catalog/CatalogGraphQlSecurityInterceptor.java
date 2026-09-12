package dev.tailrocks.catalog;

import graphql.GraphQLContext;
import org.springframework.graphql.server.WebGraphQlInterceptor;
import org.springframework.graphql.server.WebGraphQlRequest;
import org.springframework.graphql.server.WebGraphQlResponse;
import org.springframework.http.HttpHeaders;
import reactor.core.publisher.Mono;

import java.util.Arrays;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;

final class CatalogGraphQlSecurityInterceptor implements WebGraphQlInterceptor {
    private static final String BEARER_PREFIX = "Bearer ";
    static final String NO_STORE_CONTEXT_KEY =
        CatalogGraphQlSecurityInterceptor.class.getName() + ".noStore";
    private final String configuredToken;

    CatalogGraphQlSecurityInterceptor(String configuredToken) {
        this.configuredToken = configuredToken == null ? "" : configuredToken.trim();
    }

    @Override
    public Mono<WebGraphQlResponse> intercept(WebGraphQlRequest request, Chain chain) {
        CatalogAdminIdentity identity = authenticate(
            request.getHeaders().getFirst("Authorization")
        );
        CatalogRequestIdentity requestIdentity = CatalogRequestIdentity.fromHeaders(
            request.getHeaders()
        );
        boolean noStore = hasNoStore(request.getHeaders());
        request.configureExecutionInput((input, builder) -> builder
            .graphQLContext(context -> {
                context.put(CatalogRequestIdentity.CONTEXT_KEY, requestIdentity);
                context.put(NO_STORE_CONTEXT_KEY, noStore);
                if (identity != null) {
                    context.put(CatalogAdminIdentity.CONTEXT_KEY, identity);
                }
            })
            .build());
        return chain.next(request);
    }

    static boolean hasNoStore(HttpHeaders headers) {
        if (headers == null) {
            return false;
        }
        return headers.getOrEmpty(HttpHeaders.CACHE_CONTROL).stream()
            .flatMap(value -> Arrays.stream(value.split(",")))
            .map(String::trim)
            .anyMatch("no-store"::equalsIgnoreCase);
    }

    static boolean isNoStore(GraphQLContext context) {
        return context != null && Boolean.TRUE.equals(context.get(NO_STORE_CONTEXT_KEY));
    }

    CatalogAdminIdentity authenticate(String authorization) {
        if (configuredToken.isBlank()
            || authorization == null
            || !authorization.startsWith(BEARER_PREFIX)) {
            return null;
        }
        byte[] presented = authorization.substring(BEARER_PREFIX.length())
            .getBytes(StandardCharsets.UTF_8);
        byte[] expected = configuredToken.getBytes(StandardCharsets.UTF_8);
        if (!MessageDigest.isEqual(presented, expected)) {
            return null;
        }
        try {
            return CatalogAdminIdentity.fromConfiguredToken(configuredToken);
        } catch (IllegalArgumentException error) {
            return null;
        }
    }
}

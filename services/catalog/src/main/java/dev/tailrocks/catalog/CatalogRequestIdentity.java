package dev.tailrocks.catalog;

import graphql.GraphQLContext;
import org.springframework.http.HttpHeaders;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/** Request-scoped tenant identity collected before GraphQL execution. */
record CatalogRequestIdentity(String tenantId, String error) {
    static final String CONTEXT_KEY = CatalogRequestIdentity.class.getName();
    private static final String TENANT_BAGGAGE_KEY = "tenant.id";

    static CatalogRequestIdentity fromHeaders(HttpHeaders headers) {
        List<String> candidates = new ArrayList<>();
        try {
            for (String name : List.of("x-tenant-id", "tenant-id")) {
                for (String value : headers.getOrEmpty(name)) {
                    addCandidate(candidates, value);
                }
            }
            for (String header : headers.getOrEmpty("baggage")) {
                for (String member : header.split(",")) {
                    int separator = member.indexOf('=');
                    if (separator < 0
                        || !TENANT_BAGGAGE_KEY.equals(member.substring(0, separator).trim())) {
                        continue;
                    }
                    String value = member.substring(separator + 1)
                        .split(";", 2)[0]
                        .trim();
                    addCandidate(candidates, decodeBaggageValue(value));
                }
            }
            return fromCandidates(candidates);
        } catch (IllegalArgumentException error) {
            return new CatalogRequestIdentity(null, error.getMessage());
        }
    }

    static String resolve(GraphQLContext context, String requestedTenant) {
        return resolve(context, requestedTenant, null);
    }

    static String resolve(
        GraphQLContext context,
        String requestedTenant,
        String authenticatedTenant
    ) {
        List<String> candidates = new ArrayList<>();
        CatalogRequestIdentity inbound = fromContext(context);
        if (inbound != null) {
            if (inbound.error() != null) {
                throw new IllegalArgumentException(inbound.error());
            }
            if (inbound.tenantId() != null) {
                candidates.add(inbound.tenantId());
            }
        }
        addCandidateIfPresent(candidates, requestedTenant);
        addCandidateIfPresent(candidates, authenticatedTenant);
        return fromCandidates(candidates).tenantIdOrThrow();
    }

    static String requireTenantValue(String value) {
        return normalize(value);
    }

    private static CatalogRequestIdentity fromContext(GraphQLContext context) {
        if (context == null) {
            return null;
        }
        Object value = context.get(CONTEXT_KEY);
        return value instanceof CatalogRequestIdentity identity ? identity : null;
    }

    private static CatalogRequestIdentity fromCandidates(List<String> candidates) {
        if (candidates.isEmpty()) {
            return new CatalogRequestIdentity(null, "tenant identity is required");
        }
        String first = candidates.getFirst();
        if (candidates.stream().anyMatch(candidate -> !first.equals(candidate))) {
            return new CatalogRequestIdentity(null, "tenant identity sources conflict");
        }
        return new CatalogRequestIdentity(first, null);
    }

    private String tenantIdOrThrow() {
        if (error != null) {
            throw new IllegalArgumentException(error);
        }
        return tenantId;
    }

    private static void addCandidateIfPresent(List<String> candidates, String value) {
        if (value != null) {
            addCandidate(candidates, value);
        }
    }

    private static void addCandidate(List<String> candidates, String value) {
        String normalized = normalize(value);
        candidates.add(normalized);
    }

    private static String normalize(String value) {
        if (value == null) {
            throw new IllegalArgumentException("tenant identity is invalid");
        }
        String normalized = value.trim();
        if (normalized.isEmpty()
            || normalized.length() > 128
            || normalized.chars().anyMatch(Character::isISOControl)) {
            throw new IllegalArgumentException("tenant identity is invalid");
        }
        return normalized;
    }

    private static String decodeBaggageValue(String value) {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        ByteArrayOutputStream decoded = new ByteArrayOutputStream(bytes.length);
        for (int index = 0; index < bytes.length; index++) {
            if (bytes[index] != '%') {
                decoded.write(bytes[index]);
                continue;
            }
            if (index + 2 >= bytes.length) {
                throw new IllegalArgumentException("tenant identity is invalid");
            }
            int high = hex(bytes[index + 1]);
            int low = hex(bytes[index + 2]);
            if (high < 0 || low < 0) {
                throw new IllegalArgumentException("tenant identity is invalid");
            }
            decoded.write((high << 4) | low);
            index += 2;
        }
        return new String(decoded.toByteArray(), StandardCharsets.UTF_8);
    }

    private static int hex(byte value) {
        return switch (value) {
            case '0', '1', '2', '3', '4', '5', '6', '7', '8', '9' -> value - '0';
            case 'a', 'b', 'c', 'd', 'e', 'f' -> value - 'a' + 10;
            case 'A', 'B', 'C', 'D', 'E', 'F' -> value - 'A' + 10;
            default -> -1;
        };
    }
}

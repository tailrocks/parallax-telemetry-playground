package dev.tailrocks.catalog;

import graphql.GraphQLContext;

record CatalogAdminIdentity(String tenantId) {
    static final String CONTEXT_KEY = CatalogAdminIdentity.class.getName();
    static final String TOKEN_PREFIX = "catalog-admin:";

    CatalogAdminIdentity {
        if (tenantId == null || tenantId.isBlank()) {
            throw new IllegalStateException(
                "catalog administrator identity does not contain a tenant"
            );
        }
    }

    static CatalogAdminIdentity require(GraphQLContext context) {
        if (context == null) {
            throw new IllegalStateException(
                "catalog price updates require an authenticated catalog administrator"
            );
        }
        Object value = context.get(CONTEXT_KEY);
        if (!(value instanceof CatalogAdminIdentity identity)) {
            throw new IllegalStateException(
                "catalog price updates require an authenticated catalog administrator"
            );
        }
        return identity;
    }

    static CatalogAdminIdentity fromConfiguredToken(String token) {
        if (token == null || !token.startsWith(TOKEN_PREFIX)) {
            throw new IllegalArgumentException("catalog admin token has an invalid identity");
        }
        int secretSeparator = token.indexOf(':', TOKEN_PREFIX.length());
        if (secretSeparator < 0 || secretSeparator == TOKEN_PREFIX.length()
            || secretSeparator == token.length() - 1) {
            throw new IllegalArgumentException("catalog admin token has an invalid identity");
        }
        return new CatalogAdminIdentity(token.substring(TOKEN_PREFIX.length(), secretSeparator));
    }
}

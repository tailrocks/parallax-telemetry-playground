package dev.tailrocks.fulfillment;

import io.opentelemetry.context.Context;
import jakarta.annotation.PostConstruct;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.beans.factory.annotation.Value;

/**
 * Authenticated operational seam for publishing and verifying an existing durable order.
 *
 * The caller must send either {@code Authorization: Bearer <FULFILLMENT_INTERNAL_TOKEN>} or
 * {@code X-Fulfillment-Internal-Token: <FULFILLMENT_INTERNAL_TOKEN>}, plus a tenant query
 * parameter or matching {@code X-Tenant-Id} header. The shared token is deliberately not
 * defaulted; the application refuses to start if it is absent.
 */
@RestController
final class OrderEventController {
    static final String INTERNAL_TOKEN_HEADER = "X-Fulfillment-Internal-Token";
    static final String TENANT_HEADER = "X-Tenant-Id";
    private static final Set<String> NOT_READY_ORDER_STATUSES = Set.of("cancelled", "refunded");
    private static final Set<String> NOT_READY_SHIPMENT_STATUSES = Set.of("cancelled", "returned");

    private final FulfillmentRepository repository;
    private final OrderEventPublisher publisher;
    private final String internalToken;

    @Autowired
    OrderEventController(
        FulfillmentRepository repository,
        OrderEventPublisher publisher,
        @Value("${FULFILLMENT_INTERNAL_TOKEN:}") String internalToken
    ) {
        this.repository = repository;
        this.publisher = publisher;
        this.internalToken = internalToken == null ? "" : internalToken;
    }

    @PostConstruct
    void validateInternalToken() {
        if (internalToken.isBlank()) {
            throw new IllegalStateException(
                "FULFILLMENT_INTERNAL_TOKEN must be configured for fulfillment operational endpoints"
            );
        }
    }

    @PostMapping("/publish")
    ResponseEntity<Map<String, String>> publish(
        @RequestParam(name = "order", required = false) String orderId,
        @RequestParam(name = "tenant", required = false) String tenantId,
        @RequestHeader(name = "Authorization", required = false) String authorization,
        @RequestHeader(name = INTERNAL_TOKEN_HEADER, required = false) String headerToken,
        @RequestHeader(name = TENANT_HEADER, required = false) String tenantHeader
    ) {
        if (internalToken.isBlank()) {
            return ResponseEntity.status(HttpStatus.SERVICE_UNAVAILABLE).body(Map.of(
                "error", "internal_endpoint_not_configured"
            ));
        }
        if (!authorized(authorization, headerToken)) {
            return ResponseEntity.status(HttpStatus.UNAUTHORIZED)
                .header("WWW-Authenticate", "Bearer")
                .body(Map.of("error", "unauthorized"));
        }
        if (orderId == null || orderId.isBlank()) {
            return ResponseEntity.badRequest().body(Map.of("error", "order is required"));
        }
        TenantSelection tenant = selectTenant(tenantId, tenantHeader);
        if (!tenant.valid()) {
            return ResponseEntity.badRequest().body(Map.of("error", tenant.error()));
        }
        return repository.loadOrderEvent(tenant.tenantId(), orderId.trim())
            .map(event -> {
                publisher.publish(event, Context.current());
                return ResponseEntity.ok(Map.of(
                    "event_key", event.eventKey(),
                    "event_type", event.eventType(),
                    "order_id", event.orderId(),
                    "status", "published"
                ));
            })
            .orElseGet(() -> ResponseEntity.status(HttpStatus.NOT_FOUND).body(
                Map.of("error", "order not found", "order_id", orderId.trim())
            ));
    }

    @GetMapping("/verify/order")
    ResponseEntity<Map<String, Object>> verify(
        @RequestParam(name = "order", required = false) String orderId,
        @RequestParam(name = "tenant", required = false) String tenantId,
        @RequestHeader(name = "Authorization", required = false) String authorization,
        @RequestHeader(name = INTERNAL_TOKEN_HEADER, required = false) String headerToken,
        @RequestHeader(name = TENANT_HEADER, required = false) String tenantHeader
    ) {
        if (internalToken.isBlank()) {
            return ResponseEntity.status(HttpStatus.SERVICE_UNAVAILABLE).body(Map.of(
                "error", "internal_endpoint_not_configured"
            ));
        }
        if (!authorized(authorization, headerToken)) {
            return ResponseEntity.status(HttpStatus.UNAUTHORIZED)
                .header("WWW-Authenticate", "Bearer")
                .body(Map.of("error", "unauthorized"));
        }
        if (orderId == null || orderId.isBlank()) {
            return ResponseEntity.badRequest().body(Map.of("error", "order is required"));
        }
        TenantSelection tenant = selectTenant(tenantId, tenantHeader);
        if (!tenant.valid()) {
            return ResponseEntity.badRequest().body(Map.of("error", tenant.error()));
        }
        Optional<Map<String, Object>> result = repository.verifyOrder(
            tenant.tenantId(),
            orderId.trim()
        );
        if (result.isEmpty()) {
            return ResponseEntity.status(HttpStatus.NOT_FOUND).body(Map.of(
                "error", "order not found",
                "order_id", orderId.trim()
            ));
        }
        Map<String, Object> response = new HashMap<>(result.get());
        response.put("ready", isReady(response));
        return ResponseEntity.ok(response);
    }

    private static boolean isReady(Map<String, Object> response) {
        Object orderStatus = response.get("order_status");
        Object shipmentStatus = response.get("shipment_status");
        Object deliveries = response.get("notification_deliveries");
        return "completed".equals(response.get("fulfillment_status"))
            && response.get("shipment_id") != null
            && shipmentStatus != null
            && !NOT_READY_ORDER_STATUSES.contains(String.valueOf(orderStatus))
            && !NOT_READY_SHIPMENT_STATUSES.contains(String.valueOf(shipmentStatus))
            && deliveries instanceof Number number
            && number.longValue() > 0
            && "delivered".equals(response.get("notification_status"))
            && hasText(response.get("notification_channel"))
            && hasConfiguredProvider(response.get("notification_provider"))
            && hasText(response.get("notification_acknowledgement"));
    }

    private static boolean hasConfiguredProvider(Object value) {
        return value instanceof String provider
            && !provider.isBlank()
            && !"missing".equals(provider)
            && !"unconfigured".equals(provider);
    }

    private static boolean hasText(Object value) {
        return value instanceof String text && !text.isBlank() && !"missing".equals(text);
    }

    private boolean authorized(String authorization, String headerToken) {
        String bearer = bearerToken(authorization);
        String explicit = headerToken == null ? "" : headerToken.trim();
        boolean hasBearer = authorization != null && !authorization.isBlank();
        boolean hasExplicit = headerToken != null && !headerToken.isBlank();
        if (!hasBearer && !hasExplicit) {
            return false;
        }
        if (hasBearer && bearer.isBlank()) {
            return false;
        }
        if (hasBearer && !constantTimeEquals(internalToken, bearer)) {
            return false;
        }
        return !hasExplicit || constantTimeEquals(internalToken, explicit);
    }

    private static String bearerToken(String authorization) {
        if (authorization == null) {
            return "";
        }
        String value = authorization.trim();
        int separator = value.indexOf(' ');
        if (separator != "Bearer".length()
            || !value.regionMatches(true, 0, "Bearer", 0, "Bearer".length())) {
            return "";
        }
        return value.substring(separator + 1).trim();
    }

    private static boolean constantTimeEquals(String expected, String actual) {
        return MessageDigest.isEqual(
            expected.getBytes(StandardCharsets.UTF_8),
            actual.getBytes(StandardCharsets.UTF_8)
        );
    }

    private static TenantSelection selectTenant(String queryTenant, String headerTenant) {
        String query = clean(queryTenant);
        String header = clean(headerTenant);
        if (query.isBlank() && header.isBlank()) {
            return TenantSelection.invalid("tenant is required");
        }
        if (!query.isBlank() && !header.isBlank() && !query.equals(header)) {
            return TenantSelection.invalid("tenant query and header do not match");
        }
        return TenantSelection.valid(query.isBlank() ? header : query);
    }

    private static String clean(String value) {
        return value == null ? "" : value.trim();
    }

    private record TenantSelection(String tenantId, String error) {
        static TenantSelection valid(String tenantId) {
            return new TenantSelection(tenantId, "");
        }

        static TenantSelection invalid(String error) {
            return new TenantSelection("", error);
        }

        boolean valid() {
            return error.isBlank();
        }
    }
}

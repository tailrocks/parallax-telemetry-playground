package dev.tailrocks.payment;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Objects;
import java.util.Optional;

/** Stable identities shared by payment idempotency and checkout recovery. */
final class PaymentIdentity {
    static final String AUTHORIZE_REQUEST_SUFFIX = ":authorize";

    private PaymentIdentity() {}

    static String stablePaymentId(String tenantId, String authorizeRequestId) {
        return "pay_" + sha256(tenantId, authorizeRequestId);
    }

    static String stableProviderReference(String provider, String paymentId) {
        return provider + "_" + paymentId;
    }

    static Optional<String> checkoutRequestId(String authorizeRequestId) {
        if (!authorizeRequestId.endsWith(AUTHORIZE_REQUEST_SUFFIX)) {
            return Optional.empty();
        }
        String checkoutRequestId = authorizeRequestId.substring(
            0,
            authorizeRequestId.length() - AUTHORIZE_REQUEST_SUFFIX.length()
        );
        return checkoutRequestId.isBlank() ? Optional.empty() : Optional.of(checkoutRequestId);
    }

    private static String sha256(String tenantId, String authorizeRequestId) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            updatePart(digest, Objects.requireNonNull(tenantId, "tenantId"));
            updatePart(digest, Objects.requireNonNull(authorizeRequestId, "authorizeRequestId"));
            return HexFormat.of().formatHex(digest.digest());
        } catch (NoSuchAlgorithmException error) {
            throw new IllegalStateException("SHA-256 is required by the JDK", error);
        }
    }

    private static void updatePart(MessageDigest digest, String value) {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        digest.update(ByteBuffer.allocate(Integer.BYTES).putInt(bytes.length).array());
        digest.update(bytes);
    }
}

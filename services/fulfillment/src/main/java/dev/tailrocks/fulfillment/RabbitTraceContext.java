package dev.tailrocks.fulfillment;

import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.baggage.Baggage;
import io.opentelemetry.api.baggage.BaggageEntry;
import io.opentelemetry.api.common.AttributeKey;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanBuilder;
import io.opentelemetry.api.trace.SpanContext;
import io.opentelemetry.api.trace.SpanKind;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.propagation.TextMapGetter;
import io.opentelemetry.context.propagation.TextMapPropagator;
import io.opentelemetry.context.propagation.TextMapSetter;
import io.tailrocks.semconv.Semconv;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.regex.Pattern;
import org.springframework.amqp.core.MessageProperties;
import org.springframework.http.HttpHeaders;

/** Explicit W3C carrier handling for AMQP and the reverse notification hop. */
final class RabbitTraceContext {
    private static final String DEFAULT_TRACESTATE = "playground=commerce";
    private static final int MAX_BAGGAGE_VALUE_BYTES = 128;
    private static final int MAX_TRACESTATE_HEADER_BYTES = 512;
    private static final int MAX_TRACESTATE_MEMBERS = 32;
    private static final int MAX_TRACESTATE_MEMBER_BYTES = 256;
    private static final int MAX_BAGGAGE_HEADER_BYTES = 2_048;
    private static final int MAX_BAGGAGE_MEMBERS = 32;
    private static final int MAX_ENCODED_BAGGAGE_VALUE_BYTES = 384;
    private static final Pattern TRACEPARENT_PATTERN = Pattern.compile(
        "00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}"
    );
    private static final Set<String> W3C_HEADERS = Set.of(
        "traceparent",
        "tracestate",
        "baggage"
    );
    private static final Set<String> SAFE_BAGGAGE_KEYS = Set.of(
        "tenant.id",
        "user.tier",
        "customer.segment",
        "region",
        "request.priority",
        "feature.variant",
        "session.id",
        "cli.invocation.id"
    );
    private static final Set<String> BUSINESS_BAGGAGE_KEYS = Set.of(
        "tenant.id",
        "user.tier",
        "customer.segment",
        "region",
        "request.priority",
        "session.id"
    );
    private static final TextMapSetter<MessageProperties> MESSAGE_SETTER =
        (carrier, key, value) -> carrier.setHeader(key, value);
    private static final TextMapGetter<MessageProperties> MESSAGE_GETTER =
        new TextMapGetter<>() {
            @Override
            public Iterable<String> keys(MessageProperties carrier) {
                return carrier == null
                    ? Collections.emptyList()
                    : carrier.getHeaders().keySet();
            }

            @Override
            public String get(MessageProperties carrier, String key) {
                if (carrier == null) {
                    return null;
                }
                return headerValue(carrier.getHeaders(), key);
            }
        };

    private static final TextMapSetter<HttpHeaders> HTTP_SETTER =
        (carrier, key, value) -> carrier.set(key, value);

    private RabbitTraceContext() {}

    static Context extract(MessageProperties properties) {
        require(properties);
        Context extracted = propagator().extract(Context.root(), properties, MESSAGE_GETTER);
        if (!Span.fromContext(extracted).getSpanContext().isValid()) {
            throw new PermanentEventException("RabbitMQ traceparent could not be extracted");
        }
        return sanitize(extracted);
    }

    /**
     * Preserve the exact validated Rabbit carrier on the async span link.
     *
     * The extracted context establishes propagation and the link identifies
     * the producer. Keeping the carrier on the link lets downstream systems
     * verify that the relationship came from the actual message headers,
     * without copying those headers into the business event payload.
     */
    static Attributes linkAttributes(MessageProperties properties) {
        require(properties);
        return Attributes.builder()
            .put(AttributeKey.stringKey("traceparent"), requiredHeader(properties, "traceparent"))
            .put(AttributeKey.stringKey("tracestate"), requiredHeader(properties, "tracestate"))
            .put(AttributeKey.stringKey("baggage"), requiredHeader(properties, "baggage"))
            .build();
    }

    /**
     * Own the span for the listener's business processing.
     *
     * Spring AMQP and the Java agent can create separate delivery and listener
     * spans around the callback. The callback must not mutate whichever span
     * happens to be current: create the application consumer span here, before
     * entering its scope, so its name, parent, kind, and producer link belong
     * to one span with a well-defined lifecycle.
     */
    static Span startConsumerSpan(
        String destination,
        Context extracted,
        MessageProperties properties
    ) {
        if (destination == null || destination.isBlank()) {
            throw new PermanentEventException("RabbitMQ consumer destination is required");
        }
        if (extracted == null) {
            throw new PermanentEventException("RabbitMQ extracted trace context is required");
        }
        require(properties);
        SpanContext producer = Span.fromContext(extracted).getSpanContext();
        Attributes carrier = linkAttributes(properties);
        SpanBuilder builder = GlobalOpenTelemetry
            .getTracer("dev.tailrocks.fulfillment")
            .spanBuilder(destination.trim() + " process")
            .setParent(extracted)
            .setSpanKind(SpanKind.CONSUMER);
        Baggage.fromContext(extracted).forEach((key, entry) -> {
            if (BUSINESS_BAGGAGE_KEYS.contains(key)) {
                builder.setAttribute(key, entry.getValue());
            }
        });
        if (producer.isValid()) {
            builder.addLink(producer, carrier);
        }
        return builder.startSpan();
    }

    static Span startPublishSpan(
        String destination,
        String messageId,
        String eventType,
        Context parent
    ) {
        requireText(destination, "RabbitMQ producer destination");
        requireText(messageId, "RabbitMQ producer message id");
        requireText(eventType, "RabbitMQ producer event type");
        if (parent == null || !Span.fromContext(parent).getSpanContext().isValid()) {
            throw new PermanentEventException("RabbitMQ publisher requires a valid trace context");
        }
        String destinationName = destination.trim();
        SpanBuilder builder = GlobalOpenTelemetry
            .getTracer("dev.tailrocks.fulfillment")
            .spanBuilder(destinationName + " publish")
            .setParent(parent)
            .setSpanKind(SpanKind.PRODUCER)
            .setAttribute(Semconv.OTEL_KIND, Semconv.SPAN_KIND_PRODUCER)
            .setAttribute(Semconv.MESSAGING_SYSTEM, "rabbitmq")
            .setAttribute(Semconv.MESSAGING_DESTINATION_NAME, destinationName)
            .setAttribute(Semconv.MESSAGING_OPERATION_NAME, "send")
            .setAttribute(Semconv.MESSAGING_MESSAGE_ID, messageId.trim())
            .setAttribute("commerce.event.type", eventType.trim());
        Baggage.fromContext(parent).forEach((key, entry) -> {
            if (BUSINESS_BAGGAGE_KEYS.contains(key)) {
                builder.setAttribute(key, entry.getValue());
            }
        });
        return builder.startSpan();
    }

    static void inject(Context context, MessageProperties properties) {
        if (properties == null) {
            throw new PermanentEventException("RabbitMQ message properties are required");
        }
        Context sanitized = sanitize(context);
        validateInheritedBaggage(properties, sanitized);
        removeW3cHeaders(properties);
        propagator().inject(sanitized, properties, MESSAGE_SETTER);
        canonicalizeInjectedBaggage(properties);
        addDefaultTracestate(properties);
    }

    static void injectRequired(Context context, MessageProperties properties) {
        if (context == null || !Span.fromContext(context).getSpanContext().isValid()) {
            throw new PermanentEventException("RabbitMQ publisher requires a valid trace context");
        }
        inject(context, properties);
        require(properties);
    }

    private static void requireText(String value, String label) {
        if (value == null || value.isBlank()) {
            throw new PermanentEventException(label + " is required");
        }
    }

    static void require(MessageProperties properties) {
        if (properties == null) {
            throw new PermanentEventException("RabbitMQ message properties are required");
        }
        String traceparent = requiredHeader(properties, "traceparent");
        String tracestate = requiredHeader(properties, "tracestate");
        String baggage = requiredHeader(properties, "baggage");
        validateTraceparent(traceparent);
        validateTracestate(tracestate);
        validateBaggage(baggage);
    }

    static void inject(Context context, HttpHeaders headers) {
        headers.remove("traceparent");
        headers.remove("tracestate");
        headers.remove("baggage");
        propagator().inject(sanitize(context), headers, HTTP_SETTER);
        addDefaultTracestate(headers);
    }

    private static void addDefaultTracestate(MessageProperties properties) {
        String traceparent = headerValue(properties.getHeaders(), "traceparent");
        String tracestate = headerValue(properties.getHeaders(), "tracestate");
        if (traceparent != null && !traceparent.isBlank()
            && (tracestate == null || tracestate.isBlank())) {
            properties.setHeader("tracestate", DEFAULT_TRACESTATE);
        }
    }

    private static void addDefaultTracestate(HttpHeaders headers) {
        String traceparent = headers.getFirst("traceparent");
        String tracestate = headers.getFirst("tracestate");
        if (traceparent != null && !traceparent.isBlank()
            && (tracestate == null || tracestate.isBlank())) {
            headers.set("tracestate", DEFAULT_TRACESTATE);
        }
    }

    private static Context sanitize(Context context) {
        var builder = Baggage.builder();
        Baggage.fromContext(context).forEach((key, entry) -> {
            if (safeEntry(key, entry)) {
                builder.put(key, entry.getValue(), entry.getMetadata());
            }
        });
        return context.with(builder.build());
    }

    private static void validateInheritedBaggage(
        MessageProperties properties,
        Context sanitized
    ) {
        String inherited = inheritedBaggage(properties);
        if (inherited == null) {
            return;
        }
        Map<String, BaggageMember> inheritedMembers = parseBaggage(inherited);
        Baggage canonical = Baggage.fromContext(sanitized);
        for (BaggageMember member : inheritedMembers.values()) {
            BaggageEntry canonicalEntry = canonical.getEntry(member.key());
            if (canonicalEntry != null
                && !canonicalEntry.getValue().equals(member.decodedValue())) {
                throw new PermanentEventException(
                    "RabbitMQ baggage header conflicts with canonical baggage"
                );
            }
        }
    }

    private static String inheritedBaggage(MessageProperties properties) {
        List<String> values = new ArrayList<>();
        boolean found = false;
        for (Map.Entry<String, Object> header : properties.getHeaders().entrySet()) {
            if (header.getKey() != null && header.getKey().equalsIgnoreCase("baggage")) {
                found = true;
                appendBaggageHeaderValue(header.getValue(), values);
            }
        }
        return found ? String.join(",", values) : null;
    }

    private static void appendBaggageHeaderValue(Object value, List<String> values) {
        if (value instanceof Iterable<?> items) {
            for (Object item : items) {
                values.add(asBaggageHeaderValue(item));
            }
            return;
        }
        values.add(asBaggageHeaderValue(value));
    }

    private static String asBaggageHeaderValue(Object value) {
        if (value instanceof byte[] bytes) {
            for (byte character : bytes) {
                if ((character & 0x80) != 0) {
                    throw new PermanentEventException(
                        "RabbitMQ baggage header is not valid ASCII"
                    );
                }
            }
            return new String(bytes, StandardCharsets.US_ASCII);
        }
        if (value instanceof String text) {
            return text;
        }
        throw new PermanentEventException("RabbitMQ baggage header has an invalid value");
    }

    private static void canonicalizeInjectedBaggage(MessageProperties properties) {
        String baggage = headerValue(properties.getHeaders(), "baggage");
        if (baggage == null || baggage.isBlank()) {
            return;
        }
        Map<String, BaggageMember> members = parseBaggage(baggage);
        properties.setHeader(
            "baggage",
            members.values().stream()
                .map(BaggageMember::canonicalValue)
                .collect(java.util.stream.Collectors.joining(","))
        );
    }

    private static boolean safeEntry(String key, BaggageEntry entry) {
        String value = entry.getValue();
        return SAFE_BAGGAGE_KEYS.contains(key)
            && value.getBytes(StandardCharsets.UTF_8).length <= MAX_BAGGAGE_VALUE_BYTES
            && value.chars().noneMatch(Character::isISOControl);
    }

    private static void removeW3cHeaders(MessageProperties properties) {
        properties.getHeaders().keySet().removeIf(RabbitTraceContext::isW3cHeader);
    }

    private static boolean isW3cHeader(String key) {
        return key != null && W3C_HEADERS.stream().anyMatch(key::equalsIgnoreCase);
    }

    private static TextMapPropagator propagator() {
        return GlobalOpenTelemetry.getPropagators().getTextMapPropagator();
    }

    private static String headerValue(Map<String, Object> headers, String key) {
        Object value = headers.get(key);
        if (value instanceof byte[] bytes) {
            return new String(bytes, StandardCharsets.US_ASCII);
        }
        return value == null ? null : value.toString();
    }

    private static String requiredHeader(MessageProperties properties, String name) {
        String value = headerValue(properties.getHeaders(), name);
        if (value == null || value.isBlank()) {
            throw new PermanentEventException("RabbitMQ " + name + " header is required");
        }
        return trimOws(value);
    }

    private static void validateTraceparent(String value) {
        if (!TRACEPARENT_PATTERN.matcher(value).matches()
            || value.substring(3, 35).chars().allMatch(character -> character == '0')
            || value.substring(36, 52).chars().allMatch(character -> character == '0')) {
            throw new PermanentEventException("RabbitMQ traceparent header is invalid");
        }
    }

    private static void validateTracestate(String value) {
        if (value.getBytes(StandardCharsets.UTF_8).length > MAX_TRACESTATE_HEADER_BYTES) {
            throw new PermanentEventException("RabbitMQ tracestate header is too large");
        }
        String[] rawMembers = value.split(",", -1);
        if (rawMembers.length == 0 || rawMembers.length > MAX_TRACESTATE_MEMBERS) {
            throw new PermanentEventException("RabbitMQ tracestate header has invalid members");
        }
        Set<String> seen = new HashSet<>();
        for (String rawMember : rawMembers) {
            String member = trimOws(rawMember);
            int separator = member.indexOf('=');
            if (member.isBlank()
                || separator <= 0
                || separator == member.length() - 1
                || member.indexOf('=', separator + 1) >= 0) {
                throw new PermanentEventException("RabbitMQ tracestate header has an invalid member");
            }
            String key = member.substring(0, separator);
            String memberValue = member.substring(separator + 1);
            if (member.getBytes(StandardCharsets.UTF_8).length > MAX_TRACESTATE_MEMBER_BYTES
                || !validTracestateKey(key)
                || memberValue.getBytes(StandardCharsets.UTF_8).length > MAX_TRACESTATE_MEMBER_BYTES
                || !validTracestateValue(memberValue)
                || !seen.add(key)) {
                throw new PermanentEventException("RabbitMQ tracestate header has an invalid member");
            }
        }
    }

    private static boolean validTracestateKey(String value) {
        int separator = value.indexOf('@');
        if (separator < 0) {
            return validKeyPart(value, 256, false);
        }
        return separator == value.lastIndexOf('@')
            && validKeyPart(value.substring(0, separator), 241, false)
            && validKeyPart(value.substring(separator + 1), 14, false);
    }

    private static boolean validKeyPart(String value, int maxLength, boolean allowDot) {
        if (value.isEmpty() || value.length() > maxLength || !isLowerAlphaNumeric(value.charAt(0))) {
            return false;
        }
        for (int index = 1; index < value.length(); index++) {
            char character = value.charAt(index);
            if (!isLowerAlphaNumeric(character)
                && character != '_'
                && character != '*'
                && character != '/'
                && character != '-'
                && (!allowDot || character != '.')) {
                return false;
            }
        }
        return true;
    }

    private static boolean validTracestateValue(String value) {
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            boolean allowed = character >= 0x20 && character <= 0x2b
                || character >= 0x2d && character <= 0x3c
                || character >= 0x3e && character <= 0x7e;
            if (!allowed || (index == value.length() - 1 && character == ' ')) {
                return false;
            }
        }
        return !value.isEmpty();
    }

    private static Map<String, BaggageMember> parseBaggage(String value) {
        if (value.getBytes(StandardCharsets.UTF_8).length > MAX_BAGGAGE_HEADER_BYTES) {
            throw new PermanentEventException("RabbitMQ baggage header is too large");
        }
        String[] rawMembers = value.split(",", -1);
        if (rawMembers.length == 0 || rawMembers.length > MAX_BAGGAGE_MEMBERS) {
            throw new PermanentEventException("RabbitMQ baggage header has invalid members");
        }
        Map<String, BaggageMember> members = new LinkedHashMap<>();
        Set<String> seen = new HashSet<>();
        for (String rawMember : rawMembers) {
            String member = trimOws(rawMember);
            int separator = member.indexOf('=');
            if (member.isBlank() || separator <= 0 || separator == member.length() - 1) {
                throw new PermanentEventException("RabbitMQ baggage header has an invalid member");
            }
            String key = trimOws(member.substring(0, separator));
            String[] valueAndMetadata = member.substring(separator + 1).split(";", -1);
            String encodedValue = trimOws(valueAndMetadata[0]);
            if (!validBaggageKey(key)
                || !validEncodedBaggageValue(encodedValue)
                || !seen.add(key)) {
                throw new PermanentEventException("RabbitMQ baggage header has an invalid member");
            }
            String decodedValue = decodeBaggageValue(encodedValue);
            for (int index = 1; index < valueAndMetadata.length; index++) {
                String metadata = trimOws(valueAndMetadata[index]);
                int metadataSeparator = metadata.indexOf('=');
                if (metadataSeparator <= 0
                    || metadataSeparator == metadata.length() - 1
                    || !validBaggageKey(trimOws(metadata.substring(0, metadataSeparator)))
                    || !validEncodedBaggageValue(trimOws(metadata.substring(metadataSeparator + 1)))) {
                    throw new PermanentEventException("RabbitMQ baggage header has an invalid member");
                }
            }
            StringBuilder canonicalValue = new StringBuilder(key)
                .append('=').append(encodedValue);
            for (int index = 1; index < valueAndMetadata.length; index++) {
                canonicalValue.append(';').append(trimOws(valueAndMetadata[index]));
            }
            members.put(key, new BaggageMember(key, decodedValue, canonicalValue.toString()));
        }
        return members;
    }

    private static void validateBaggage(String value) {
        parseBaggage(value);
    }

    private static String decodeBaggageValue(String value) {
        byte[] encoded = value.getBytes(StandardCharsets.US_ASCII);
        byte[] decoded = new byte[encoded.length];
        int decodedLength = 0;
        for (int index = 0; index < encoded.length; index++) {
            if (encoded[index] != '%') {
                decoded[decodedLength++] = encoded[index];
                continue;
            }
            decoded[decodedLength++] = (byte) (hexValue(encoded[++index]) * 16
                + hexValue(encoded[++index]));
        }
        try {
            return StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(decoded, 0, decodedLength))
                .toString();
        } catch (CharacterCodingException error) {
            throw new PermanentEventException("RabbitMQ baggage header has an invalid member");
        }
    }

    private static int hexValue(byte character) {
        if (character >= '0' && character <= '9') {
            return character - '0';
        }
        if (character >= 'a' && character <= 'f') {
            return character - 'a' + 10;
        }
        return character - 'A' + 10;
    }

    private record BaggageMember(String key, String decodedValue, String canonicalValue) {}

    private static boolean validBaggageKey(String value) {
        int separator = value.indexOf('@');
        if (separator < 0) {
            return validBaggageKeyPart(value, 256);
        }
        return separator == value.lastIndexOf('@')
            && validBaggageKeyPart(value.substring(0, separator), 256)
            && validBaggageKeyPart(value.substring(separator + 1), 14);
    }

    private static boolean validBaggageKeyPart(String value, int maxLength) {
        if (value.isEmpty() || value.length() > maxLength || !isLowerAlphaNumeric(value.charAt(0))) {
            return false;
        }
        for (int index = 1; index < value.length(); index++) {
            char character = value.charAt(index);
            if (!isLowerAlphaNumeric(character) && character != '.' && character != '_' && character != '-') {
                return false;
            }
        }
        return true;
    }

    private static boolean validEncodedBaggageValue(String value) {
        if (value.isEmpty()
            || value.getBytes(StandardCharsets.UTF_8).length > MAX_ENCODED_BAGGAGE_VALUE_BYTES) {
            return false;
        }
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            if (character == '%') {
                if (index + 2 >= value.length()
                    || !isHex(value.charAt(index + 1))
                    || !isHex(value.charAt(index + 2))) {
                    return false;
                }
                index += 2;
                continue;
            }
            boolean allowed = character >= 0x21 && character <= 0x2b
                || character >= 0x2d && character <= 0x3c
                || character >= 0x3e && character <= 0x7e;
            if (!allowed) {
                return false;
            }
        }
        return true;
    }

    private static boolean isLowerAlphaNumeric(char character) {
        return character >= 'a' && character <= 'z'
            || character >= '0' && character <= '9';
    }

    private static boolean isHex(char character) {
        return character >= '0' && character <= '9'
            || character >= 'a' && character <= 'f'
            || character >= 'A' && character <= 'F';
    }

    private static String trimOws(String value) {
        int start = 0;
        int end = value.length();
        while (start < end && (value.charAt(start) == ' ' || value.charAt(start) == '\t')) {
            start++;
        }
        while (end > start && (value.charAt(end - 1) == ' ' || value.charAt(end - 1) == '\t')) {
            end--;
        }
        return value.substring(start, end);
    }
}

package io.tailrocks.testsupport;

import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.StatusCode;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.Scope;
import io.opentelemetry.context.propagation.TextMapGetter;
import io.tailrocks.semconv.Semconv;
import java.lang.reflect.Method;
import java.util.Map;
import org.junit.jupiter.api.extension.ExtensionContext;
import org.junit.jupiter.api.extension.InvocationInterceptor;
import org.junit.jupiter.api.extension.ReflectiveInvocationContext;

/** Emits the shared test wire contract while preserving JUnit's original outcome. */
public final class OpenTelemetryTestExtension implements InvocationInterceptor {
    private static final TextMapGetter<Map<String, String>> CARRIER = new TextMapGetter<>() {
        @Override
        public Iterable<String> keys(Map<String, String> carrier) {
            return carrier.keySet();
        }

        @Override
        public String get(Map<String, String> carrier, String key) {
            return carrier.get(key);
        }
    };

    @Override
    public void interceptTestMethod(
        Invocation<Void> invocation,
        ReflectiveInvocationContext<Method> method,
        ExtensionContext context
    ) throws Throwable {
        intercept(invocation, method, context);
    }

    @Override
    public void interceptTestTemplateMethod(
        Invocation<Void> invocation,
        ReflectiveInvocationContext<Method> method,
        ExtensionContext context
    ) throws Throwable {
        intercept(invocation, method, context);
    }

    private static void intercept(
        Invocation<Void> invocation,
        ReflectiveInvocationContext<Method> method,
        ExtensionContext context
    ) throws Throwable {
        String suite = context.getRequiredTestClass().getName();
        String name = suite + "#" + method.getExecutable().getName();
        Span span = GlobalOpenTelemetry.get().getTracer("playground.junit")
            .spanBuilder("test.case")
            .setParent(parentContext())
            .startSpan();
        span.setAttribute(Semconv.TEST_SUITE_NAME, suite);
        span.setAttribute(Semconv.TEST_CASE_NAME, name);
        span.setAttribute(Semconv.TEST_SUITE_RUN_STATUS, "pass");
        span.setAttribute(Semconv.CICD_PIPELINE_TASK_TYPE, "test");
        span.setAttribute(Semconv.CICD_PIPELINE_RUN_ID, System.getenv().getOrDefault("PARALLAX_RUN_ID", ""));
        span.setAttribute(Semconv.PARALLAX_TEST_ID, testId(name));
        span.setAttribute("test.configuration.os", System.getProperty("os.name"));
        span.setAttribute("test.configuration.environment", System.getenv().getOrDefault("PARALLAX_TEST_ENVIRONMENT", "local"));
        if (!context.getDisplayName().equals(method.getExecutable().getName())) {
            span.setAttribute("test.case.parameters", context.getDisplayName());
        }
        try (Scope ignored = span.makeCurrent()) {
            invocation.proceed();
            span.setAttribute(Semconv.TEST_CASE_RESULT_STATUS, "pass");
        } catch (Throwable failure) {
            span.setAttribute(Semconv.TEST_CASE_RESULT_STATUS, "fail");
            span.setAttribute("test.case.failure.kind", failure instanceof AssertionError ? "assertion_failure" : "harness_error");
            span.recordException(failure);
            span.setStatus(StatusCode.ERROR, failure.getMessage() == null ? failure.getClass().getSimpleName() : failure.getMessage());
            throw failure;
        } finally {
            span.end();
        }
    }

    private static Context parentContext() {
        String traceparent = System.getenv("TRACEPARENT");
        if (traceparent == null || traceparent.isBlank()) {
            return Context.current();
        }
        return GlobalOpenTelemetry.get().getPropagators().getTextMapPropagator()
            .extract(Context.root(), Map.of("traceparent", traceparent), CARRIER);
    }

    private static String testId(String fallback) {
        String override = System.getenv("PARALLAX_TEST_ID");
        return override == null || override.isBlank() ? fallback : override;
    }
}

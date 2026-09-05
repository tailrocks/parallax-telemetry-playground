package dev.tailrocks.catalog;

import dev.openfeature.sdk.BooleanHook;
import dev.openfeature.sdk.FlagEvaluationDetails;
import dev.openfeature.sdk.HookContext;
import dev.openfeature.sdk.StringHook;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.trace.Span;

import java.util.Map;
import java.util.Optional;

final class BooleanFeatureFlagSpanHook implements BooleanHook {
    @Override
    public void after(
        HookContext<Boolean> context,
        FlagEvaluationDetails<Boolean> details,
        Map<String, Object> hints
    ) {
        Span.current().addEvent("feature_flag.evaluation", Attributes.builder()
            .put("feature_flag.key", context.getFlagKey())
            .put("feature_flag.provider_name", "flagd")
            .put("feature_flag.variant", Optional.ofNullable(details.getVariant()).orElse(""))
            .put("feature_flag.value", Boolean.TRUE.equals(details.getValue()))
            .build());
    }

    @Override
    public void error(HookContext<Boolean> context, Exception error, Map<String, Object> hints) {
        Span.current().addEvent("feature_flag.evaluation", Attributes.builder()
            .put("feature_flag.key", context.getFlagKey())
            .put("feature_flag.provider_name", "flagd")
            .put("feature_flag.variant", "error")
            .put("feature_flag.error", error.getClass().getSimpleName())
            .build());
    }
}

final class StringFeatureFlagSpanHook implements StringHook {
    @Override
    public void after(
        HookContext<String> context,
        FlagEvaluationDetails<String> details,
        Map<String, Object> hints
    ) {
        Span.current().addEvent("feature_flag.evaluation", Attributes.builder()
            .put("feature_flag.key", context.getFlagKey())
            .put("feature_flag.provider_name", "flagd")
            .put("feature_flag.variant", Optional.ofNullable(details.getVariant()).orElse(""))
            .put("feature_flag.value", Optional.ofNullable(details.getValue()).orElse(""))
            .build());
    }

    @Override
    public void error(HookContext<String> context, Exception error, Map<String, Object> hints) {
        Span.current().addEvent("feature_flag.evaluation", Attributes.builder()
            .put("feature_flag.key", context.getFlagKey())
            .put("feature_flag.provider_name", "flagd")
            .put("feature_flag.variant", "error")
            .put("feature_flag.error", error.getClass().getSimpleName())
            .build());
    }
}

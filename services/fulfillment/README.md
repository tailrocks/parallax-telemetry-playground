# fulfillment (Java, Spring Boot) — scaffold

Spring Boot **broker consumer**. Consumes from the broker (CONSUMER span + span link to the producer), then calls the Rust `notifications` service over HTTP (the reverse Java→Rust hop).

Instrumentation: Sentry OpenTelemetry agent (`-javaagent:sentry-opentelemetry-agent.jar`,
`SENTRY_AUTO_INIT=false`) + Sentry Spring Boot starter. W3C trace context over
broker message headers.
Build mirrors `services/catalog/build.gradle.kts`. Finalize per
docs/research/validation/telemetry-playground-sample-project.md §8.

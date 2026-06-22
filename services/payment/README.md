# payment (Java, Spring Boot) — scaffold

Spring Boot + **gRPC** (spring-grpc). Java gRPC server quoted by checkout over gRPC; JVM runtime metrics + Micrometer exemplars; deliberate GC/CPU/failure chaos.

Instrumentation: Sentry OpenTelemetry agent (`-javaagent:sentry-opentelemetry-agent.jar`,
`SENTRY_AUTO_INIT=false`) + Sentry Spring Boot starter. W3C trace context over
gRPC metadata.
Build mirrors `services/catalog/build.gradle.kts`. Finalize per
docs/research/validation/telemetry-playground-sample-project.md §8.

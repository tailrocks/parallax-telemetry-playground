# Shared Dockerfile for the Spring Boot (Java) services. Parameterized by SERVICE
# (the services/<SERVICE> directory). Each service is its own Gradle build with
# its own wrapper.
#
# Instrumentation: the UPSTREAM OpenTelemetry Java agent (OTLP export to the
# lab's Rotel → fan-out to every backend). We deliberately do NOT use Sentry's
# `sentry-opentelemetry-agent`: it replaces the exporter pipeline and prevents
# the fan-out backends from seeing Java signals. Each service instead includes
# the Spring Boot 4-compatible Sentry SDK starter, which emits Sentry envelopes
# while this upstream agent remains the sole OTLP instrumentation/export path.
ARG SERVICE
ARG OTEL_AGENT_VERSION=2.30.0
ARG OTEL_AGENT_SHA256=9d6bc2ad8dd8fb7f730984988e57b8ac0a82d81c7b3b8ae795378718733a509d
ARG GRPC_HEALTH_PROBE_VERSION=v0.4.56

FROM eclipse-temurin:25-jdk@sha256:e787e08ef76f4c16866108cd7f9fcd96a68eef3ac6cc76866897d4d02d5a2262 AS build
ARG SERVICE
# Mirror the repo layout so a service's repo-relative source dirs resolve the
# same in the image as in the checkout (e.g. payment's protobuf srcDir
# "../../proto" → services/<svc> up two levels to the shared proto/).
WORKDIR /src
COPY services/${SERVICE} /src/services/${SERVICE}
# The generated shared semconv sources are included by every Java service
# via srcDir("../semconv/src/main/java").
COPY services/semconv /src/services/semconv
COPY proto /src/proto
WORKDIR /src/services/${SERVICE}
RUN ./gradlew --no-daemon bootJar

FROM eclipse-temurin:25-jre@sha256:f9e65324a37f28209ce7dd0e5149a7aa954520ed936fb87813cf6ded2400a112 AS run
ARG SERVICE
ARG OTEL_AGENT_VERSION
ARG OTEL_AGENT_SHA256
ARG GRPC_HEALTH_PROBE_VERSION
ARG TARGETARCH
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl netcat-openbsd; \
    case "${TARGETARCH}" in \
      amd64) probe_arch=amd64; probe_sha256=dc13e24d92cdd05d1eb9faf7192c65057dc5b52d38b01aa56188e6899604ec93 ;; \
      arm64) probe_arch=arm64; probe_sha256=1f84fc307ca65c0eb59c83a81202544e1005614735acd96a89beba67abb03c63 ;; \
      *) echo "unsupported TARGETARCH for grpc health probe: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    probe_name="grpc_health_probe-linux-${probe_arch}"; \
    curl --fail --silent --show-error --location --retry 3 \
      --output "/tmp/${probe_name}" \
      "https://github.com/grpc-ecosystem/grpc-health-probe/releases/download/${GRPC_HEALTH_PROBE_VERSION}/${probe_name}"; \
    printf '%s  %s\n' "${probe_sha256}" "/tmp/${probe_name}" | sha256sum -c -; \
    install -m 0755 "/tmp/${probe_name}" /usr/local/bin/grpc_health_probe; \
    rm -f "/tmp/${probe_name}"; \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
# Upstream OpenTelemetry Java agent — auto-instruments Spring MVC/GraphQL/gRPC/
# JDBC/RabbitMQ and exports OTLP per the OTEL_* env (set per-service in the compose:
# OTLP/gRPC to Rotel :4317 — retested 2026-08-14 with javaagent 2.30.0 /
# Rotel v0.2.5; HTTP/protobuf :4318 remains the documented fallback).
RUN set -eux; \
    otel_agent_url="https://repo1.maven.org/maven2/io/opentelemetry/javaagent/opentelemetry-javaagent/${OTEL_AGENT_VERSION}/opentelemetry-javaagent-${OTEL_AGENT_VERSION}.jar"; \
    curl --fail --silent --show-error --location --retry 3 \
      --output /app/otel-agent.jar "$otel_agent_url"; \
    printf '%s  %s\n' "$OTEL_AGENT_SHA256" /app/otel-agent.jar | sha256sum -c -
COPY --from=build /src/services/${SERVICE}/build/libs/*.jar /app/app.jar
ENV JAVA_TOOL_OPTIONS="-javaagent:/app/otel-agent.jar" \
    OTEL_PROPAGATORS="tracecontext,baggage"
ENTRYPOINT ["java", "-jar", "/app/app.jar"]

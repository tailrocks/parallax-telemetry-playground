# Shared Dockerfile for the Spring Boot (Java) services. Parameterized by SERVICE
# (the services/<SERVICE> directory). Each service is its own Gradle build with
# its own wrapper.
#
# Instrumentation: the UPSTREAM OpenTelemetry Java agent (OTLP export to the
# lab's Rotel → fan-out to every backend). We deliberately do NOT use Sentry's
# `sentry-opentelemetry-agent`: it installs Sentry's own SpanProcessor and emits
# nothing over OTLP even with OTEL_TRACES_EXPORTER=otlp set, so the fan-out
# backends never see the Java services (reproduced live 2026-06-23 — the
# LoggingSpanExporter printed zero spans, no span reached any backend). OTLP
# fan-out is the lab's whole point; Sentry still receives the Java traces/logs
# as a Rotel OTLP backend. (A Sentry *SDK envelope* path for Java is separately
# blocked upstream: the Sentry Spring Boot starter 8.44 references the relocated
# RestClientCustomizer and does not load on Spring Boot 4.x.)
ARG SERVICE
ARG JDK=25
ARG OTEL_AGENT_VERSION=2.29.0

FROM eclipse-temurin:${JDK}-jdk AS build
ARG SERVICE
# Mirror the repo layout so a service's repo-relative source dirs resolve the
# same in the image as in the checkout (e.g. payment's protobuf srcDir
# "../../proto" → services/<svc> up two levels to the shared proto/).
WORKDIR /src
COPY services/${SERVICE} /src/services/${SERVICE}
COPY proto /src/proto
COPY graphql /src/graphql
WORKDIR /src/services/${SERVICE}
RUN ./gradlew --no-daemon bootJar

FROM eclipse-temurin:${JDK}-jre AS run
ARG SERVICE
ARG OTEL_AGENT_VERSION
WORKDIR /app
# Upstream OpenTelemetry Java agent — auto-instruments Spring MVC/GraphQL/gRPC/
# JDBC/Kafka and exports OTLP per the OTEL_* env (set per-service in the compose:
# OTLP/HTTP to Rotel :4318, since the agent's gRPC sender can't read Rotel's gRPC
# response).
ADD https://repo1.maven.org/maven2/io/opentelemetry/javaagent/opentelemetry-javaagent/${OTEL_AGENT_VERSION}/opentelemetry-javaagent-${OTEL_AGENT_VERSION}.jar /app/otel-agent.jar
COPY --from=build /src/services/${SERVICE}/build/libs/*.jar /app/app.jar
ENV JAVA_TOOL_OPTIONS="-javaagent:/app/otel-agent.jar" \
    OTEL_PROPAGATORS="tracecontext,baggage"
ENTRYPOINT ["java", "-jar", "/app/app.jar"]

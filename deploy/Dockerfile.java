# Shared Dockerfile for the Spring Boot (Java) services. Parameterized by SERVICE
# (the services/<SERVICE> directory). Each service is its own Gradle build with
# its own wrapper.
#
# One agent for both OTel and Sentry: io.sentry:sentry-opentelemetry-agent run as
# -javaagent with SENTRY_AUTO_INIT=false; the Spring Boot starter inits the SDK
# (spec §8). Agent version MUST equal the Sentry SDK version (sentry-bom 8.44.0).
ARG SERVICE
ARG JDK=25
ARG SENTRY_AGENT_VERSION=8.44.0

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
ARG SENTRY_AGENT_VERSION
WORKDIR /app
# OTel + Sentry agent (single javaagent). Pinned to the SDK version.
ADD https://repo1.maven.org/maven2/io/sentry/sentry-opentelemetry-agent/${SENTRY_AGENT_VERSION}/sentry-opentelemetry-agent-${SENTRY_AGENT_VERSION}.jar /app/sentry-otel-agent.jar
COPY --from=build /src/services/${SERVICE}/build/libs/*.jar /app/app.jar
# The sentry-opentelemetry agent owns BOTH OTel auto-instrumentation and Sentry
# SDK init (auto-inits from SENTRY_DSN). The Sentry Spring Boot starter 8.44 is
# incompatible with Spring Boot 4.x (references the relocated
# org.springframework.boot.web.client.RestClientCustomizer), so we do not use it
# — the agent is the single init point. With no DSN the agent no-ops gracefully.
ENV SENTRY_AUTO_INIT=true \
    JAVA_TOOL_OPTIONS="-javaagent:/app/sentry-otel-agent.jar" \
    OTEL_PROPAGATORS="tracecontext,baggage"
ENTRYPOINT ["java", "-jar", "/app/app.jar"]

// Spring Boot + GraphQL catalog service. Instrumented zero-code by the Sentry
// OpenTelemetry agent (run with -javaagent:sentry-opentelemetry-agent.jar and
// SENTRY_AUTO_INIT=false); the Sentry Spring Boot starter inits the SDK.
plugins {
    java
    id("org.springframework.boot") version "4.1.0"
    id("io.spring.dependency-management") version "1.1.7"
    // id("io.sentry.jvm.gradle") version "5.0.0" // source context upload
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(21) } }
repositories { mavenCentral() }
dependencies {
    implementation("org.springframework.boot:spring-boot-starter-graphql")
    implementation("org.springframework.boot:spring-boot-starter-web")
    // A7: GraphQL-over-WebSocket transport for the priceChanges subscription.
    implementation("org.springframework.boot:spring-boot-starter-websocket")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    // Sentry is initialized by the sentry-opentelemetry javaagent. The Sentry
    // Spring Boot starter 8.44 is incompatible with Spring Boot 4.x (references
    // the relocated org.springframework.boot.web.client.RestClientCustomizer),
    // so it is intentionally omitted; the agent owns OTel + Sentry init.
    implementation("dev.openfeature:sdk:1.21.0")
    implementation("dev.openfeature.contrib.providers:flagd:0.14.0")
}

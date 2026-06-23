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
    implementation(platform("io.sentry:sentry-bom:8.44.1"))
    implementation("io.sentry:sentry-spring-boot-starter-jakarta")
    implementation("dev.openfeature:sdk:1.21.0")
    implementation("dev.openfeature.contrib.providers:flagd:0.14.0")
}

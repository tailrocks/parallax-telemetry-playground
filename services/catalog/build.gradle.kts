// Spring Boot + GraphQL catalog service. The upstream OpenTelemetry Java agent
// preserves OTLP fan-out while the Sentry Spring Boot starter owns Sentry SDK
// envelopes and exception capture.
plugins {
    java
    id("org.springframework.boot") version "4.1.0"
    id("io.spring.dependency-management") version "1.1.7"
    id("com.atkinsondev.opentelemetry-build") version "4.6.2"
    // id("io.sentry.jvm.gradle") version "5.0.0" // source context upload
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(25) } }
sourceSets { main { java { srcDir("../semconv/src/main/java") } } }
repositories { mavenCentral() }
dependencies {
    implementation("org.springframework.boot:spring-boot-starter-graphql")
    implementation("org.springframework.boot:spring-boot-starter-web")
    // A7: GraphQL-over-WebSocket transport for the priceChanges subscription.
    implementation("org.springframework.boot:spring-boot-starter-websocket")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    implementation("io.sentry:sentry-spring-boot-starter-jakarta:8.46.0")
    implementation("dev.openfeature:sdk:1.21.0")
    implementation("dev.openfeature.contrib.providers:flagd:0.14.0")
    implementation("io.opentelemetry:opentelemetry-api")
    testImplementation("org.springframework.boot:spring-boot-starter-test")
}
openTelemetryBuild {
    endpoint = System.getenv("OTEL_EXPORTER_OTLP_ENDPOINT") ?: "http://rotel:4317"
    serviceName = "catalog-tests"
    customTags = mapOf("parallax.run.id" to (System.getenv("PARALLAX_RUN_ID") ?: ""))
    taskTraceEnvironmentEnabled = true
}
tasks.withType<Test>().configureEach {
    useJUnitPlatform()
    reports.junitXml.mergeReruns.set(true)
    environment("PARALLAX_RUN_ID", System.getenv("PARALLAX_RUN_ID") ?: "")
}

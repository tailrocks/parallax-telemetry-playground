import com.google.protobuf.gradle.proto

// Spring Boot + gRPC payment service. Generates Java stubs from the shared
// ../../proto/pricing.proto and serves the Pricing gRPC contract — the
// cross-language counterpart to the Rust pricing service.
//
// Version note (latest-stable, 2026-06-23): Spring Boot 4.1.0 + the graduated
// Spring gRPC 1.1.0 (Boot-owned `spring-boot-starter-grpc-server`). The earlier
// 4.0.0 hold is gone: Boot 4.1 absorbed Spring gRPC, so its Gradle plugin now
// registers the protobuf `grpc` locator and wires the generate tasks itself —
// the old manual `protobuf { plugins/generateProtoTasks }` block was the source
// of the "ExecutableLocator 'grpc' already exists" clash and is removed. We pin
// only protoc (4.34.2, = Boot's managed protobuf-java; grpc-java 1.80.0 is
// BOM-managed). catalog/fulfillment are already on Boot 4.1.0.
plugins {
    java
    id("org.springframework.boot") version "4.1.0"
    id("io.spring.dependency-management") version "1.1.7"
    id("com.google.protobuf") version "0.9.4"
    id("com.atkinsondev.opentelemetry-build") version "4.6.2"
    id("org.gradle.test-retry") version "1.6.5"
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(25) } }
sourceSets { main { java { srcDir("../semconv/src/main/java") } } }
repositories { mavenCentral() }
val otelJavaAgent = configurations.create("otelJavaAgent")
val testOtelEndpoint = System.getenv("OTEL_EXPORTER_OTLP_ENDPOINT")?.takeIf(String::isNotBlank)
val testResourceAttributes = listOfNotNull(
    System.getenv("OTEL_RESOURCE_ATTRIBUTES")?.takeIf(String::isNotBlank),
    "service.version=$version",
    "vcs.ref.head.revision=${System.getenv("GITHUB_SHA") ?: "local"}",
    "test.configuration.os=${System.getProperty("os.name")}",
    "test.configuration.environment=${System.getenv("PARALLAX_TEST_ENVIRONMENT") ?: "local"}",
).joinToString(",")
dependencies {
    // Spring Boot 4.1 graduated Spring gRPC: the starter is now Boot-owned and
    // split by role — payment is a gRPC server. Boot's BOM manages the
    // spring-grpc-core + grpc-java + protobuf versions, so no separate
    // spring-grpc-dependencies BOM import is needed.
    implementation("org.springframework.boot:spring-boot-starter-grpc-server")
    implementation("io.grpc:grpc-services")
    compileOnly("org.apache.tomcat:annotations-api:6.0.53")
    implementation("org.springframework.boot:spring-boot-starter")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    implementation("io.sentry:sentry-spring-boot-starter-jakarta:8.46.0")
    implementation("io.opentelemetry:opentelemetry-api")
    compileOnly("org.junit.jupiter:junit-jupiter-api")
    testImplementation("org.springframework.boot:spring-boot-starter-test")
    testImplementation("io.grpc:grpc-inprocess")
    // Keep test traces on the same upstream agent path as the deployed JVM.
    add(otelJavaAgent.name, "io.opentelemetry.javaagent:opentelemetry-javaagent:2.29.0")
}
openTelemetryBuild {
    endpoint = System.getenv("OTEL_EXPORTER_OTLP_ENDPOINT") ?: "http://rotel:4317"
    serviceName = "payment-tests"
    customTags = mapOf("parallax.run.id" to (System.getenv("PARALLAX_RUN_ID") ?: ""))
    taskTraceEnvironmentEnabled = true
}
tasks.withType<Test>().configureEach {
    useJUnitPlatform()
    reports.junitXml.mergeReruns.set(true)
    retry {
        maxRetries.set(1)
        maxFailures.set(3)
        failOnPassedAfterRetry.set(false)
    }
    inputs.files(otelJavaAgent)
    jvmArgs("-javaagent:${otelJavaAgent.singleFile.absolutePath}")
    environment("PARALLAX_RUN_ID", System.getenv("PARALLAX_RUN_ID") ?: "")
    environment("PARALLAX_TEST_ID", System.getenv("PARALLAX_TEST_ID") ?: "")
    environment("PARALLAX_TEST_ENVIRONMENT", System.getenv("PARALLAX_TEST_ENVIRONMENT") ?: "local")
    environment("TRACEPARENT", System.getenv("TRACEPARENT") ?: "")
    environment("OTEL_RESOURCE_ATTRIBUTES", testResourceAttributes)
    if (testOtelEndpoint == null) {
        environment("OTEL_TRACES_EXPORTER", "none")
        environment("OTEL_METRICS_EXPORTER", "none")
        environment("OTEL_LOGS_EXPORTER", "none")
    } else {
        environment("OTEL_EXPORTER_OTLP_ENDPOINT", testOtelEndpoint)
    }
}
// Spring Boot 4.1 graduated gRPC support: when `com.google.protobuf` is
// applied, Boot's Gradle plugin registers the `grpc` protoc locator AND
// attaches it to every generate task (re-registering either throws "already
// exists"). It does NOT pin the protoc compiler itself, so we set only that —
// matched to Boot 4.1's managed protobuf-java 4.34.2 (gencode must be <= the
// runtime, so equal is correct; grpc-java is 1.80.0, BOM-managed).
protobuf {
    protoc { artifact = "com.google.protobuf:protoc:4.34.2" }
}
sourceSets { main { proto { srcDir("../../proto") } } }

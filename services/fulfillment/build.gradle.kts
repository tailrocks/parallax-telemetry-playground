// Spring Boot broker consumer. Consumes order events (CONSUMER span + span link
// to the producer), then calls the Rust notifications service over HTTP (the
// reverse Java→Rust hop). The upstream OTel agent exports to Rotel and the
// Sentry starter captures SDK envelopes.
plugins {
    java
    id("org.springframework.boot") version "4.1.0"
    id("io.spring.dependency-management") version "1.1.7"
    id("com.google.protobuf") version "0.9.4"
    id("com.atkinsondev.opentelemetry-build") version "4.6.2"
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
    implementation("io.opentelemetry:opentelemetry-api")
    compileOnly("org.junit.jupiter:junit-jupiter-api")
    implementation("org.springframework.boot:spring-boot-starter")
    implementation("org.springframework.boot:spring-boot-starter-web")
    // Spring Boot 4 modularized auto-configuration: plain spring-kafka no longer
    // brings KafkaAutoConfiguration (KafkaTemplate + listener factories). The
    // starter pulls spring-kafka + the spring-boot-kafka autoconfig module.
    implementation("org.springframework.boot:spring-boot-starter-kafka")
    implementation("org.springframework.boot:spring-boot-starter-grpc-client")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    implementation("io.sentry:sentry-spring-boot-starter-jakarta:8.46.0")
    testImplementation("org.springframework.boot:spring-boot-starter-test")
    testImplementation("org.springframework.kafka:spring-kafka-test")
    testImplementation("io.grpc:grpc-inprocess")
    // Keep test traces on the same upstream agent path as the deployed JVM.
    add(otelJavaAgent.name, "io.opentelemetry.javaagent:opentelemetry-javaagent:2.29.0")
}
openTelemetryBuild {
    endpoint = System.getenv("OTEL_EXPORTER_OTLP_ENDPOINT") ?: "http://rotel:4317"
    serviceName = "fulfillment-tests"
    customTags = mapOf("cli.invocation.id" to (System.getenv("CLI_INVOCATION_ID") ?: ""))
    taskTraceEnvironmentEnabled = true
}
protobuf {
    protoc { artifact = "com.google.protobuf:protoc:4.34.2" }
}
tasks.withType<Test>().configureEach {
    useJUnitPlatform()
    reports.junitXml.mergeReruns.set(true)
    inputs.files(otelJavaAgent)
    jvmArgs("-javaagent:${otelJavaAgent.singleFile.absolutePath}")
    environment("CLI_INVOCATION_ID", System.getenv("CLI_INVOCATION_ID") ?: "")
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
sourceSets { main { proto { srcDir("../../proto") } } }

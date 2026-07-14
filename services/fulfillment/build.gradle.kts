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
repositories { mavenCentral() }
val otelJavaAgent by configurations.creating
dependencies {
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
    testImplementation("io.grpc:grpc-inprocess")
    // Keep test traces on the same upstream agent path as the deployed JVM.
    otelJavaAgent("io.opentelemetry.javaagent:opentelemetry-javaagent:2.29.0")
}
openTelemetryBuild {
    endpoint = System.getenv("OTEL_EXPORTER_OTLP_ENDPOINT") ?: "http://rotel:4317"
    serviceName = "fulfillment-tests"
    customTags = mapOf("parallax.run.id" to (System.getenv("PARALLAX_RUN_ID") ?: ""))
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
    environment("PARALLAX_RUN_ID", System.getenv("PARALLAX_RUN_ID") ?: "")
}
sourceSets { main { proto { srcDir("../../proto") } } }

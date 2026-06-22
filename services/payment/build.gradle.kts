// Spring Boot + gRPC payment service. This base compiles today; add the gRPC
// server by applying the `com.google.protobuf` plugin + spring-grpc starter and
// generating from ../../proto/pricing.proto (see README). Instrumented zero-code
// by the Sentry OpenTelemetry agent.
plugins {
    java
    id("org.springframework.boot") version "3.4.1"
    id("io.spring.dependency-management") version "1.1.7"
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(21) } }
repositories { mavenCentral() }
dependencies {
    implementation("org.springframework.boot:spring-boot-starter-web")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    implementation(platform("io.sentry:sentry-bom:8.44.0"))
    implementation("io.sentry:sentry-spring-boot-starter-jakarta")
    // gRPC variant (next step): org.springframework.grpc:spring-grpc-spring-boot-starter
}

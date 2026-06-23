import com.google.protobuf.gradle.id
import com.google.protobuf.gradle.proto

// Spring Boot + gRPC payment service. Generates Java stubs from the shared
// ../../proto/pricing.proto and serves the Pricing gRPC contract — the
// cross-language counterpart to the Rust pricing service.
//
// Version note (latest-stable audit, 2026-06-23): held at Spring Boot 4.0.0 +
// spring-grpc 1.0.3 + protobuf-gradle-plugin 0.9.4 on purpose. Boot 4.1.0's
// Gradle plugin double-registers the protobuf `grpc` ExecutableLocator
// ("Cannot add a ExecutableLocator with name 'grpc' ... already exists"), and
// spring-grpc 1.1.0's BOM only resolves the starter version on Boot 4.1 — so
// the two newest pins are mutually blocked here pending a protobuf-config
// restructure. protoc gencode is pinned to 4.33.4 to match the protobuf-java
// runtime the spring-grpc 1.0.3 BOM resolves (a newer gencode aborts at class
// init); grpc-java 1.82.0. catalog/fulfillment (no protobuf plugin) on Boot 4.1.0.
plugins {
    java
    id("org.springframework.boot") version "4.0.0"
    id("io.spring.dependency-management") version "1.1.7"
    id("com.google.protobuf") version "0.9.4"
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(21) } }
repositories { mavenCentral() }
dependencyManagement {
    imports { mavenBom("org.springframework.grpc:spring-grpc-dependencies:1.0.3") }
}
dependencies {
    implementation("org.springframework.grpc:spring-grpc-spring-boot-starter")
    implementation("io.grpc:grpc-services")
    compileOnly("org.apache.tomcat:annotations-api:6.0.53")
    implementation("org.springframework.boot:spring-boot-starter")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
}
protobuf {
    // protoc (gencode) must be <= the protobuf-java runtime that spring-grpc's
    // 1.0.3 BOM resolves (4.33.4); newer gencode aborts at QuoteRequest init
    // ("Runtime version cannot be older than the linked gencode version").
    protoc { artifact = "com.google.protobuf:protoc:4.33.4" }
    plugins { id("grpc") { artifact = "io.grpc:protoc-gen-grpc-java:1.82.0" } }
    generateProtoTasks { all().forEach { it.plugins { id("grpc") } } }
}
sourceSets { main { proto { srcDir("../../proto") } } }

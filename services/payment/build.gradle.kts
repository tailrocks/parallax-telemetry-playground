import com.google.protobuf.gradle.id
import com.google.protobuf.gradle.proto

// Spring Boot + gRPC payment service. Generates Java stubs from the shared
// ../../proto/pricing.proto and serves the Pricing gRPC contract — the
// cross-language counterpart to the Rust pricing service.
plugins {
    java
    id("org.springframework.boot") version "3.4.1"
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
    implementation(platform("io.sentry:sentry-bom:8.44.0"))
    implementation("io.sentry:sentry-spring-boot-starter-jakarta")
}
protobuf {
    protoc { artifact = "com.google.protobuf:protoc:4.28.3" }
    plugins { id("grpc") { artifact = "io.grpc:protoc-gen-grpc-java:1.68.1" } }
    generateProtoTasks { all().forEach { it.plugins { id("grpc") } } }
}
sourceSets { main { proto { srcDir("../../proto") } } }

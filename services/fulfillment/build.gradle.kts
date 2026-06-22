// Spring Boot broker consumer. Consumes order events (CONSUMER span + span link
// to the producer), then calls the Rust notifications service over HTTP (the
// reverse Java→Rust hop). Instrumented zero-code by the Sentry OTel agent.
plugins {
    java
    id("org.springframework.boot") version "3.4.1"
    id("io.spring.dependency-management") version "1.1.7"
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(21) } }
repositories { mavenCentral() }
dependencies {
    implementation("org.springframework.boot:spring-boot-starter")
    implementation("org.springframework.boot:spring-boot-starter-web")
    implementation("org.springframework.kafka:spring-kafka")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    implementation(platform("io.sentry:sentry-bom:8.44.0"))
    implementation("io.sentry:sentry-spring-boot-starter-jakarta")
}

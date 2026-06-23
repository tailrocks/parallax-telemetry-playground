// Spring Boot broker consumer. Consumes order events (CONSUMER span + span link
// to the producer), then calls the Rust notifications service over HTTP (the
// reverse Java→Rust hop). Instrumented zero-code by the Sentry OTel agent.
plugins {
    java
    id("org.springframework.boot") version "4.1.0"
    id("io.spring.dependency-management") version "1.1.7"
}
group = "dev.tailrocks"; version = "0.1.0"
java { toolchain { languageVersion = JavaLanguageVersion.of(25) } }
repositories { mavenCentral() }
dependencies {
    implementation("org.springframework.boot:spring-boot-starter")
    implementation("org.springframework.boot:spring-boot-starter-web")
    // Spring Boot 4 modularized auto-configuration: plain spring-kafka no longer
    // brings KafkaAutoConfiguration (KafkaTemplate + listener factories). The
    // starter pulls spring-kafka + the spring-boot-kafka autoconfig module.
    implementation("org.springframework.boot:spring-boot-starter-kafka")
    implementation("org.springframework.boot:spring-boot-starter-actuator")
    // Sentry initialized by the sentry-opentelemetry javaagent; the Spring Boot
    // starter 8.44 is incompatible with Spring Boot 4.x (RestClientCustomizer
    // relocation), so it is omitted. Agent owns OTel + Sentry init.
}

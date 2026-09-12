package dev.tailrocks.catalog;

import com.fasterxml.jackson.databind.ObjectMapper;
import dev.openfeature.contrib.providers.flagd.FlagdProvider;
import dev.openfeature.sdk.OpenFeatureAPI;
import org.springframework.context.annotation.Bean;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.graphql.server.WebGraphQlInterceptor;

@SpringBootApplication
public class CatalogApplication {
    public static void main(String[] args) {
        OpenFeatureAPI api = OpenFeatureAPI.getInstance();
        api.addHooks(new BooleanFeatureFlagSpanHook(), new StringFeatureFlagSpanHook());
        api.setProvider(new FlagdProvider());
        SpringApplication.run(CatalogApplication.class, args);
    }

    @Bean
    ObjectMapper objectMapper() {
        return new ObjectMapper().findAndRegisterModules();
    }

    @Bean
    WebGraphQlInterceptor catalogGraphQlSecurity() {
        return new CatalogGraphQlSecurityInterceptor(System.getenv("CATALOG_ADMIN_TOKEN"));
    }
}

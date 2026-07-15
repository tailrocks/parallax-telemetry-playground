package dev.tailrocks.payment;

import io.opentelemetry.api.trace.Span;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import java.io.IOException;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable;
import org.junit.jupiter.api.extension.ExtendWith;

import static org.junit.jupiter.api.Assertions.assertTrue;

/** Opt-in fixtures that create real retry chains for the W4 acceptance run. */
@ExtendWith(OpenTelemetryTestExtension.class)
@EnabledIfEnvironmentVariable(named = "PLAYGROUND_TEST_FLAKY_FIXTURE", matches = "1")
class TestTelemetryAcceptanceTest {
    @Test
    void assertion_failure_passes_on_retry() throws IOException {
        assertTrue(isRetry("assertion"), "intentional first-attempt assertion failure");
    }

    @Test
    void harness_error_passes_on_retry() throws IOException {
        if (!isRetry("harness")) {
            throw new IllegalStateException("intentional first-attempt harness error");
        }
    }

    private static boolean isRetry(String kind) throws IOException {
        String token = System.getenv("PLAYGROUND_TEST_ATTEMPT_TOKEN");
        if (token == null || token.isBlank()) {
            throw new IllegalStateException("PLAYGROUND_TEST_ATTEMPT_TOKEN is required for acceptance fixtures");
        }
        Path marker = Path.of(System.getProperty("java.io.tmpdir"), "parallax-java-flaky-" + token + "-" + kind);
        try {
            Files.createFile(marker);
            return false;
        } catch (FileAlreadyExistsException retry) {
            Files.delete(marker);
            Span.current().setAttribute("test.attempt.ordinal", 2L);
            return true;
        }
    }
}

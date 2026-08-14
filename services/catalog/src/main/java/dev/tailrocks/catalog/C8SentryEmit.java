package dev.tailrocks.catalog;

import io.sentry.Sentry;
import io.sentry.protocol.User;

/**
 * Real sentry-java (Spring starter 8.53 on the classpath) envelope for c8.
 * Not a request path — a one-shot JavaExec so Parallax can ingest a Java SDK
 * envelope without hijacking the compose DSN that still points at Sentry.
 */
public final class C8SentryEmit {
    private C8SentryEmit() {}

    public static void main(String[] args) {
        String dsn = System.getenv("SENTRY_DSN");
        if (dsn == null || dsn.isBlank()) {
            throw new IllegalStateException("SENTRY_DSN required");
        }
        Sentry.init(
                options -> {
                    options.setDsn(dsn);
                    options.setRelease("c8-java-sdk");
                    options.setEnvironment("playground");
                    options.setTracesSampleRate(0.0);
                });
        User user = new User();
        user.setId("c8-java");
        Sentry.setUser(user);
        Sentry.captureException(new IllegalStateException("c8-java-sdk PaymentError"));
        Sentry.flush(5000);
        Sentry.close();
    }
}

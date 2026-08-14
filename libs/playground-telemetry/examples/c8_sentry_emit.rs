//! Real sentry-rust 0.49 envelope to the DSN in `SENTRY_DSN` (c8).
fn main() {
    let dsn = std::env::var("SENTRY_DSN").expect("SENTRY_DSN");
    let mut opts = sentry::ClientOptions::new()
        .release("c8-rust-sdk")
        .environment("playground")
        .attach_stacktrace(true)
        .traces_sample_rate(0.0);
    opts.dsn = Some(dsn.parse().expect("SENTRY_DSN parse"));
    let _guard = sentry::init(opts);
    sentry::configure_scope(|scope| {
        scope.set_tag("c8.sdk", "sentry.rust");
        scope.set_fingerprint(Some(&["c8-rust-sdk"]));
    });
    sentry::capture_message("c8-rust-sdk PaymentError", sentry::Level::Error);
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(std::time::Duration::from_secs(5)));
    }
}

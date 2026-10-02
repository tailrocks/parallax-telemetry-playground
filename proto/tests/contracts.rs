#[test]
fn contracts_present() {
    let _ = playground_proto::payment::v1::AuthorizeRequest::default();
    let _ = playground_proto::pricing::v1::QuoteRequest::default();
}

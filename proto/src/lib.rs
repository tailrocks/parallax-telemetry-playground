//! Generated gRPC contracts shared across services.

pub mod payment {
    pub mod v1 {
        include!("gen/playground.payment.v1.rs");
    }
}

pub mod pricing {
    pub mod v1 {
        include!("gen/playground.pricing.v1.rs");
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_present() {
        let _ = payment::v1::AuthorizeRequest::default();
        let _ = pricing::v1::QuoteRequest::default();
    }
}

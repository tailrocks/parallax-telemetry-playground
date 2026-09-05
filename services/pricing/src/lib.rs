//! Pricing owns quote calculation. It reads the seeded price lists and active
//! promotions from PostgreSQL, caches complete quotes in Redis, and exposes
//! the result over a reusable tonic service.

#![allow(clippy::result_large_err)]

pub mod api;
pub(crate) mod application;
pub(crate) mod domain;
pub mod infrastructure;

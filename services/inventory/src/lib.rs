//! Inventory owns the durable stock ledger. Reservations run in PostgreSQL
//! transactions and use row locks, so the normal checkout path exercises real
//! contention and rollback behavior.

pub mod api;
mod application;
mod domain;
pub mod infrastructure;

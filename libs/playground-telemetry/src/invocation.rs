//! Neutral CLI-invocation identity (plan 158): `cli.invocation.id` /
//! `session.id` minting, ambient storage, and the env carrier.
//!
//! Ids are stamped on root spans and log events (the jackin shape) — never on
//! Resource — except for genuinely wrapped child processes, which receive the
//! id through the `CLI_INVOCATION_ID` env carrier and surface it as a
//! resource attribute (the generic wrapped-emitter path Parallax reads).

use std::sync::OnceLock;

use crate::semconv;

/// Env carrier for the invocation id. The legacy `PARALLAX_RUN_ID` carrier is
/// retired (operator, 2026-07-17) and is never read or written.
pub const INVOCATION_ENV: &str = "CLI_INVOCATION_ID";

static INVOCATION_ID: OnceLock<String> = OnceLock::new();

/// The ambient invocation id for this process: the env carrier when a parent
/// minted one (wrapped child), else a fresh UUIDv4. Minted once per process.
pub fn invocation_id() -> &'static str {
    INVOCATION_ID.get_or_init(|| {
        std::env::var(INVOCATION_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    })
}

/// A fresh interactive-session id (one ownership window inside an invocation).
#[must_use]
pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `OTEL_RESOURCE_ATTRIBUTES` value carrying the invocation id exactly once,
/// for spawning wrapped children (generic wrapped-emitter shape).
#[must_use]
pub fn resource_attrs_with_invocation_id(existing: Option<&str>, invocation_id: &str) -> String {
    let existing = existing.unwrap_or_default();
    if existing
        .split(',')
        .filter_map(|item| item.split_once('='))
        .any(|(key, _)| key.trim() == semconv::CLI_INVOCATION_ID)
    {
        return existing.to_owned();
    }
    if existing.trim().is_empty() {
        format!("{}={invocation_id}", semconv::CLI_INVOCATION_ID)
    } else {
        format!("{existing},{}={invocation_id}", semconv::CLI_INVOCATION_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_id_is_stable_within_the_process() {
        let first = invocation_id();
        let second = invocation_id();
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn session_ids_are_unique() {
        assert_ne!(new_session_id(), new_session_id());
    }

    #[test]
    fn child_resource_attrs_carry_the_id_exactly_once() {
        assert_eq!(
            resource_attrs_with_invocation_id(None, "inv-a"),
            "cli.invocation.id=inv-a"
        );
        assert_eq!(
            resource_attrs_with_invocation_id(Some("service.name=cli"), "inv-a"),
            "service.name=cli,cli.invocation.id=inv-a"
        );
        assert_eq!(
            resource_attrs_with_invocation_id(Some("cli.invocation.id=existing"), "inv-a"),
            "cli.invocation.id=existing"
        );
    }
}

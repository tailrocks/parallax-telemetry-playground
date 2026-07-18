# GitHub Actions runner policy

Every executable workflow uses the same YAML on all lanes: `velnor` is the
default on `self-hosted,velnor-target-mvp`; `github` uses pinned
`ubuntu-26.04`; and `both` runs identical steps on both. Use the canonical
inline `matrix.config` expression. Only `matrix.config.writer` may gate a
mutating step, with exactly one writer.

Rust jobs use mold and local-only sccache v0.16.0. Java uses the mise-managed
JDK and lane-scoped Gradle caches; web uses mise-managed Bun and a lane-scoped
package cache. Never add a second compiler cache or branch semantics by lane.

Every job has a timeout; every workflow has concurrency. Checkout stays
shallow with credential persistence disabled. Changes to lanes, actions,
toolchains, or caches must pass `velnor`, `github`, and `both` verification.

# Releases (regression track)

Scenario A13 models a release regression with environment-driven identity, not
fake artifacts.

- Compose starts checkout with `RELEASE=v1` for the clean phase.
- The script recreates only checkout with `RELEASE=v2`; checkout's release
  regression branch fails without `?fail=1`.
- `libs/playground-telemetry` maps `RELEASE` to `service.version` and maps
  `GIT_SHA` to `vcs.ref.head.revision` when present.
- The script restores checkout to `RELEASE=v1` before exit.

After Parallax plan 041 lands, this is the release-strip demo:
`v1` clean traffic followed by a `v2` checkout error spike.

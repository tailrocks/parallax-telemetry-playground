#!/usr/bin/env bash
# c8: real Sentry SDK envelopes (Rust 0.49, Java 8.53, JS 10.70) into Parallax.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

export SENTRY_DSN="${SENTRY_DSN:-http://c8public@127.0.0.1:4000/1}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-1.97.1}"
export C_ISOLATION
mkdir -p "$C_ISOLATION"

echo "c8 rust SDK → $SENTRY_DSN"
(
  cd "$ROOT"
  mise exec -- cargo run --locked -p playground-telemetry --example c8_sentry_emit
)

echo "c8 java SDK → $SENTRY_DSN"
(
  cd "$ROOT/services/catalog"
  ./gradlew --no-daemon c8SentryEmit
)

echo "c8 js SDK → $SENTRY_DSN"
(
  cd "$ROOT/web"
  bun ../scenarios/c8-emit-js.ts
)

found_rust=0 found_java=0 found_js=0
for _ in $(seq 1 30); do
  data="$(c_gql '{ issues(limit: 50) { items { fingerprint title errorType } } }')"
  printf '%s' "$data" >"$C_ISOLATION/c8-issues.json"
  eval "$(python3 - <<'PY'
import json
d=json.load(open(__import__("os").environ["C_ISOLATION"]+"/c8-issues.json"))
items=(d.get("issues") or {}).get("items") or []
b=json.dumps(items).lower()
print(f"found_rust={int('c8-rust-sdk' in b)}")
print(f"found_java={int('c8-java-sdk' in b)}")
print(f"found_js={int('c8-js-sdk' in b)}")
PY
)"
  if [[ "$found_rust" == 1 && "$found_java" == 1 && "$found_js" == 1 ]]; then
    break
  fi
  sleep 1
done

echo "c8 rust=$found_rust java=$found_java js=$found_js"
if [[ "$found_rust" != 1 || "$found_java" != 1 || "$found_js" != 1 ]]; then
  echo "c8: missing SDK issue(s) rust=$found_rust java=$found_java js=$found_js" >&2
  python3 -c "import json; print(json.dumps(json.load(open('$C_ISOLATION/c8-issues.json')), indent=2)[:2000])" >&2 || true
  exit 1
fi
echo "c8 ok rust+java+js envelopes ingested"

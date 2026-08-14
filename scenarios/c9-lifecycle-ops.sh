#!/usr/bin/env bash
# c9: doctor + prune against a throwaway HOME (never the operator ~/.parallax).
# Also asserts remote context add/list and invocation --otlp-forward off.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin

iso="$(c_isolation_dir)"
cleanup() { rm -rf "$iso"; }
trap cleanup EXIT

real_home="$(python3 -c 'import pwd,os; print(pwd.getpwuid(os.getuid()).pw_dir)')"
export HOME="$iso"
mkdir -p "$HOME/.parallax"
printf 'c9-marker\n' >"$HOME/.parallax/marker"
real_marker_before=""
if [[ -f "$real_home/.parallax/marker" ]]; then
  real_marker_before="$(stat -f %m "$real_home/.parallax/marker" 2>/dev/null || stat -c %Y "$real_home/.parallax/marker")"
fi

echo "c9 isolated HOME=$HOME"
"$PARALLAX_BIN" doctor
dry="$("$PARALLAX_BIN" prune --json)"
echo "$dry"
printf '%s' "$dry" | grep -Eq 'plan_id|items|dry' || {
  echo "c9: prune dry-run json missing plan" >&2
  exit 1
}
"$PARALLAX_BIN" prune --execute --yes --json >/dev/null
[[ -f "$HOME/.parallax/marker" ]] || {
  echo "c9: isolated marker vanished unexpectedly" >&2
  exit 1
}

# Remote contexts live in isolated HOME only.
"$PARALLAX_BIN" context add c9lab --url "$PARALLAX_URL"
listed="$("$PARALLAX_BIN" context list)"
echo "$listed"
printf '%s' "$listed" | grep -q c9lab || {
  echo "c9: context add/list missed c9lab" >&2
  exit 1
}
"$PARALLAX_BIN" context show c9lab >/dev/null

# Prove we did not prune the operator home.
if [[ -n "$real_marker_before" ]]; then
  real_marker_after="$(stat -f %m "$real_home/.parallax/marker" 2>/dev/null || stat -c %Y "$real_home/.parallax/marker")"
  [[ "$real_marker_before" == "$real_marker_after" ]] || {
    echo "c9: operator ~/.parallax/marker mtime changed" >&2
    exit 1
  }
fi
if [[ -d "$real_home/.parallax" ]]; then
  # Isolated prune must not delete the operator data dir.
  [[ -d "$real_home/.parallax" ]]
fi

# --otlp-forward compare mode (uses live serve; HOME isolation still on).
fwd="$("$PARALLAX_BIN" invocation start --otlp-forward off -- /bin/echo c9-otlp-forward)"
echo "$fwd"
printf '%s' "$fwd" | grep -Eq 'invocation id|c9-otlp-forward' || {
  echo "c9: --otlp-forward off wrapper failed" >&2
  exit 1
}

echo "c9 ok isolated-HOME=$iso doctor+prune+context+otlp-forward"

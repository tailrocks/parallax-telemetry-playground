#!/usr/bin/env bash
# c3: SSE live tail returns at least one row after a log/trace seed.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
# Open the stream first, then emit so the live subscriber sees the event.
curl -sS -N --max-time 8 "$PARALLAX_URL/v1/traces/stream" >"$tmp" &
pid=$!
sleep 0.4
"$SCRIPT_DIR/c1-issue-context.sh" >/tmp/c3-c1.out || true
wait "$pid" || true
if [[ ! -s "$tmp" ]]; then
  echo "c3: SSE traces stream produced no bytes" >&2
  exit 1
fi
echo "c3 ok bytes=$(wc -c < "$tmp")"

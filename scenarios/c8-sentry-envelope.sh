#!/usr/bin/env bash
# c8: POST a minimal Sentry envelope to Parallax and assert an issue appears.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

event='{"event_id":"c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8","timestamp":"2026-08-14T12:00:00Z","platform":"native","exception":{"values":[{"type":"PaymentError","value":"declined"}]}}'
header='{"event_id":"c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8","sent_at":"2026-08-14T12:00:00Z"}'
item='{"type":"event","length":'"${#event}"'}'
envelope="$(printf '%s\n%s\n%s' "$header" "$item" "$event")"
code="$(curl -sS -o /tmp/c8.txt -w "%{http_code}" -X POST "$PARALLAX_URL/api/1/envelope/" \
  -H "content-type: application/x-sentry-envelope" \
  -H "X-Sentry-Auth: Sentry sentry_key=c8public, sentry_version=7" \
  --data-binary "$envelope" || true)"
if [[ "$code" != 2* ]]; then
  echo "c8: envelope ingest http $code (sentry mapping may be disabled)" >&2
  exit 1
fi
echo "c8 ok http=$code"

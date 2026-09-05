#!/usr/bin/env bash
# A18: plant the current Sentry envelope redaction corpus (fake
# email/token/card/jwt) and compare what each backend stores.
set -euo pipefail
BASE="${PARALLAX_URL:-http://127.0.0.1:4000}"
event_id="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
event="{\"event_id\":\"$event_id\",\"timestamp\":\"2026-09-04T00:00:00Z\",\"platform\":\"python\",\"message\":\"a18 redaction corpus\",\"extra\":{\"email\":\"alice@example.com\",\"token\":\"sk-live-CANARY1234567890\",\"card\":\"4111111111111111\",\"jwt\":\"eyJhbGciOiJIUzI1NiJ9.CANARY.sig\"}}"
header="{\"event_id\":\"$event_id\",\"sent_at\":\"2026-09-04T00:00:00Z\"}"
item="{\"type\":\"event\",\"length\":${#event}}"
envelope="$(printf '%s\n%s\n%s' "$header" "$item" "$event")"
code="$(curl --max-time 15 -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/1/envelope/" \
  -H 'content-type: application/x-sentry-envelope' \
  -H 'X-Sentry-Auth: Sentry sentry_key=c8public, sentry_version=7' \
  --data-binary "$envelope")"
[[ "$code" =~ ^2 ]] || { echo "A18 envelope ingest failed: HTTP $code" >&2; exit 1; }
echo "A18 Sentry envelope planted [HTTP $code] event=$event_id"
echo "Compare: does Parallax redact the canary fields in issue, bundle, MCP, webhook, Sentry ack, and UI output?"

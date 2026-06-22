#!/usr/bin/env bash
# A18: plant a redaction canary corpus (fake email/token/card/jwt) in span attrs
# + log body, then compare what each backend stores RAW vs SCRUBS.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
curl -sS "$BASE/checkout?canary=1" -o /dev/null -w "canary planted [%{http_code}]\n"
echo "compare: do Parallax/Sentry redact the canary fields while others store raw?"

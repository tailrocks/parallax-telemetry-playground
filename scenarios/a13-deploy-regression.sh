#!/usr/bin/env bash
# A13: deploy + regression. Run clean (RELEASE=v1), then a regressed build
# (RELEASE=v2 with PAYMENT_FAILURE=1) to compare release-attributed regression.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "v1 (clean):";    curl -sS "$BASE/checkout" -o /dev/null -w " [%{http_code}]\n"
echo "v2 (regressed):"; curl -sS "$BASE/checkout?fail=1" -o /dev/null -w " [%{http_code}]\n"
echo "deploy/release markers + commit sha are emitted as resource attrs (RELEASE env)."

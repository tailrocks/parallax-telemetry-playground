#!/usr/bin/env bash
# Run one real checkout and assert its distributed topology through Parallax.
# Usage: parallax invocation start -- scripts/verify-commerce-trace.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
checkout_url="${CHECKOUT_URL:-http://localhost:8088}"
parallax_api_url="${PARALLAX_API_URL:-http://127.0.0.1:4000}"
invocation_id="${CLI_INVOCATION_ID:?CLI_INVOCATION_ID is required; run through parallax invocation start}"
traceparent="${TRACEPARENT:?TRACEPARENT is required; run through parallax invocation start}"
if [[ ! "$traceparent" =~ ^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$ ]]; then
  echo "invalid traceparent: expected complete version-00 W3C value" >&2
  exit 2
fi
trace_id="${traceparent:3:32}"
span_id="${traceparent:36:16}"
if [[ "$trace_id" =~ ^0{32}$ || "$span_id" =~ ^0{16}$ ]]; then
  echo "invalid traceparent: trace and span identifiers cannot be zero" >&2
  exit 2
fi

tracestate="${TRACESTATE:-playground=commerce}"
if ! LC_ALL=C awk -v header="$tracestate" '
  function trim_ows(value) {
    sub(/^[ \t]*/, "", value)
    sub(/[ \t]*$/, "", value)
    return value
  }
  BEGIN {
    if (length(header) == 0 || length(header) > 512) exit 1
    count = split(header, members, ",")
    if (count < 1 || count > 32) exit 1
    for (member_index = 1; member_index <= count; member_index += 1) {
      member = trim_ows(members[member_index])
      if (member == "" || member ~ /^[ \t]+$/) exit 1
      separator = index(member, "=")
      if (separator <= 1) exit 1
      if (index(substr(member, separator + 1), "=") != 0) exit 1
      key = substr(member, 1, separator - 1)
      value = substr(member, separator + 1)
      if (key !~ /^([a-z0-9][a-z0-9_*\/-]{0,255}|[a-z0-9][a-z0-9_*\/-]{0,240}@[a-z0-9][a-z0-9_*\/-]{0,13})$/) exit 1
      if (value !~ /^[ -~]*[!-~]$/ || value ~ /[,=]/ || length(value) > 256) exit 1
      if (key in seen) exit 1
      seen[key] = 1
    }
    exit 0
  }
' <<<"$tracestate"; then
  echo "invalid tracestate: expected bounded, unique lowercase W3C members" >&2
  exit 2
fi

baggage="${BAGGAGE:-}"
if [[ "$baggage" != *"cli.invocation.id="* ]]; then
  if [[ -n "$baggage" ]]; then
    baggage+=","
  fi
  baggage+="cli.invocation.id=$invocation_id"
fi
static_baggage="tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal"
if [[ -n "$baggage" ]]; then
  baggage+=",$static_baggage"
else
  baggage="$static_baggage"
fi
if [[ "$baggage" == ,* || "$baggage" == *, || "$baggage" == *",,"* || ${#baggage} -gt 2048 ]]; then
  echo "invalid baggage: generated W3C members are empty or over the bound" >&2
  exit 2
fi
request_id="commerce-verification-$invocation_id"

payload="$(curl --fail-with-body --max-time 30 -sS -X POST "$checkout_url/checkout" \
  -H 'content-type: application/json' \
  -H "traceparent: $traceparent" \
  -H "tracestate: $tracestate" \
  -H "baggage: $baggage" \
  --data "$(jq -cn --arg request_id "$request_id" '{tenant_id:"tenant-acme",customer_id:"customer-acme-ava",items:[{sku:"WIDGET-1",quantity:1}],currency_code:"USD",payment_method_token:"tok_visa",request_id:$request_id}')")"
jq -e '.status == "paid" and (.order_id | type == "string" and length > 0)' <<<"$payload" >/dev/null

cd "$root"
if command -v playground >/dev/null 2>&1; then
  playground commerce-verify "$trace_id" "$parallax_api_url"
elif [[ -x "$root/target/debug/playground" ]]; then
  "$root/target/debug/playground" commerce-verify "$trace_id" "$parallax_api_url"
else
  cargo run --locked -p playground-cli -- commerce-verify "$trace_id" "$parallax_api_url"
fi

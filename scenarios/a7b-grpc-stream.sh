#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_validate_parallax_mode

BASE="${CHECKOUT_URL:-http://localhost:8088}"

request() {
  local label="$1"
  local query="$2"
  local expected="$3"
  local validation="$4"
  local body
  local code
  local trace_id
  local traceparent
  body="$(mktemp "${TMPDIR:-/tmp}/a7b.XXXXXX")"
  trace_id="$(c_new_trace_id)"
  traceparent="$(c_traceparent_for "$trace_id")"
  if code="$(curl --max-time 15 -sS "$BASE/quote-stream$query" \
    -H "traceparent: $traceparent" -o "$body" -w "%{http_code}")"; then
    :
  else
    code=000
  fi

  printf "%-18s http %s: " "$label" "$code"
  cat "$body"
  printf '\n'
  [[ "$code" == "$expected" ]] || {
    echo "$label: expected HTTP $expected, got $code" >&2
    rm -f "$body"
    return 1
  }

  case "$validation" in
    clean)
      jq -e '
        (.error? | not)
        and (.streamed_quotes | type == "number" and . == 1)
        and (.cancelled == false)
      ' "$body" >/dev/null || {
        echo "$label: response did not prove one delivered quote message" >&2
        rm -f "$body"
        return 1
      }
      ;;
    unknown)
      jq -e '
        .error == "product_not_found"
        and ((.message // "") | (type == "string" and length > 0))
      ' "$body" >/dev/null || {
        echo "$label: response did not prove the pricing stream rejection" >&2
        rm -f "$body"
        return 1
      }
      ;;
    cancelled)
      jq -e '
        (.error? | not)
        and (.streamed_quotes | type == "number" and . == 1)
        and (.cancelled == true)
      ' "$body" >/dev/null || {
        echo "$label: response did not prove cancellation after a streamed message" >&2
        rm -f "$body"
        return 1
      }
      ;;
    *)
      echo "$label: unknown response validation $validation" >&2
      rm -f "$body"
      return 1
      ;;
  esac

  if c_parallax_required; then
    local trace_filter
    case "$validation" in
      clean|cancelled)
        trace_filter='any(.trace.spans[]?; .service == "checkout" and .name == "checkout.quote_stream")
          and any(.trace.spans[]?; .service == "pricing" and .name == "pricing.quote_stream.send" and
            ((try (.attributes | fromjson) catch {})["rpc.message.type"] == "SENT"))'
        ;;
      unknown)
        trace_filter='any(.trace.spans[]?; .service == "checkout" and .name == "checkout.quote_stream")
          and any(.trace.spans[]?; .service == "pricing" and .name == "pricing.quote_stream.send" and
            ((try (.attributes | fromjson) catch {})["rpc.message.type"] == "ERROR"))'
        ;;
    esac
    c_wait_for_trace "$trace_id" "$label trace" \
      "{ trace(traceId: \"$trace_id\") { spans { service name attributes } } }" \
      "$trace_filter"
  fi

  rm -f "$body"
}

echo "A7b gRPC stream events"
request "clean stream" "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=WIDGET-1&quantity=1" 200 clean
request "unknown product" "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=NO-SUCH-SKU&quantity=1" 404 unknown
request "client cancellation" "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=WIDGET-1&quantity=1&delay_ms=1" 200 cancelled

if c_parallax_required; then
  echo "A7b Parallax trace assertions PASS"
else
  echo "A7b Parallax trace assertions SKIPPED by SCENARIO_PARALLAX_MODE=skip"
fi

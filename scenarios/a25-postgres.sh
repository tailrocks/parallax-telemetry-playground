#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_validate_parallax_mode

BASE="${INVENTORY_URL:-http://localhost:8089}"
TENANT_ID="${INVENTORY_TENANT_ID:-tenant-acme}"
RUN_ID="${A25_RUN_ID:-a25-$$}"
TMPDIR="${TMPDIR:-/tmp}"
CHECKOUT_REQUEST_ID="${A25_CHECKOUT_REQUEST_ID:-${CHECKOUT_REQUEST_ID:-}}"
CHECKOUT_LEASE_TOKEN="${A25_CHECKOUT_LEASE_TOKEN:-${CHECKOUT_LEASE_TOKEN:-}}"
WORK_DIR="$(mktemp -d "$TMPDIR/a25.XXXXXX")"
HOLD_DIR="$WORK_DIR/holds"
mkdir -p "$HOLD_DIR"
trap 'rm -rf "$WORK_DIR"' EXIT

if [[ -z "$CHECKOUT_REQUEST_ID" || -z "$CHECKOUT_LEASE_TOKEN" ]]; then
  echo "A25: set A25_CHECKOUT_REQUEST_ID and A25_CHECKOUT_LEASE_TOKEN to a current checkout fence" >&2
  exit 2
fi

release_reservation() {
  local reservation_id="$1"
  local sku="$2"
  local quantity="$3"
  local location_id="$4"
  local body="$WORK_DIR/release-${reservation_id}.body"
  local payload
  local code
  payload="$(jq -cn \
    --arg tenant "$TENANT_ID" \
    --arg reservation "$reservation_id" \
    --arg sku "$sku" \
    --arg location "$location_id" \
    --arg request "$CHECKOUT_REQUEST_ID" \
    --arg token "$CHECKOUT_LEASE_TOKEN" \
    --argjson quantity "$quantity" \
    '{tenant_id:$tenant, reservation_id:$reservation, sku:$sku, quantity:$quantity,
      location_id:$location, checkout_request_id:$request, checkout_lease_token:$token}')"
  if code="$(curl --max-time 15 -sS -X POST "$BASE/release" \
    -H 'content-type: application/json' \
    -H "traceparent: $(c_traceparent_for "$(c_new_trace_id)")" \
    -o "$body" -w '%{http_code}' --data "$payload")"; then
    :
  else
    code=000
  fi
  [[ "$code" == "200" ]] && jq -e --arg reservation "$reservation_id" --arg sku "$sku" \
    --argjson quantity "$quantity" '
      .status == "released"
      and .reservation_id == $reservation
      and .sku == $sku
      and .released == $quantity
    ' "$body" >/dev/null || {
      echo "A25: release failed for $reservation_id (http $code)" >&2
      cat "$body" >&2
      return 1
    }
}

request() {
  local label="$1"
  local reservation_id="$2"
  local sku="$3"
  local expected="$4"
  local validation="$5"
  shift 5
  local -a extra=("$@")
  local body="$WORK_DIR/${reservation_id}.body"
  local trace_id
  local traceparent
  local code
  trace_id="$(c_new_trace_id)"
  traceparent="$(c_traceparent_for "$trace_id")"
  if code="$(curl --max-time 20 -sS --get "$BASE/reserve" \
    --data-urlencode "tenant_id=$TENANT_ID" \
    --data-urlencode "reservation_id=$reservation_id" \
    --data-urlencode "sku=$sku" \
    --data-urlencode 'quantity=1' \
    --data-urlencode "checkout_request_id=$CHECKOUT_REQUEST_ID" \
    --data-urlencode "checkout_lease_token=$CHECKOUT_LEASE_TOKEN" \
    "${extra[@]}" \
    -H "traceparent: $traceparent" -o "$body" -w '%{http_code}')"; then
    :
  else
    code=000
  fi
  printf "%-20s [%s] " "$label" "$code"
  cat "$body"
  printf '\n'
  [[ "$code" == "$expected" ]] || {
    echo "$label: expected HTTP $expected, got $code" >&2
    return 1
  }

  case "$validation" in
    reserved)
      jq -e --arg tenant "$TENANT_ID" --arg reservation "$reservation_id" --arg sku "$sku" '
        .tenant_id == $tenant
        and .reservation_id == $reservation
        and .sku == $sku
        and .reserved == 1
        and (.location_id | type == "string" and length > 0)
        and (.status == "reserved")
      ' "$body" >/dev/null || {
        echo "$label: response did not prove a real reservation" >&2
        return 1
      }
      local location_id
      location_id="$(jq -er '.location_id | strings | select(length > 0)' "$body")"
      release_reservation "$reservation_id" "$sku" 1 "$location_id"
      ;;
    pool)
      jq -e --arg reservation "$reservation_id" --arg sku "$sku" '
        .error == "inventory_unavailable"
        and .reservation_id == $reservation
        and .sku == $sku
      ' "$body" >/dev/null || {
        echo "$label: response did not prove a pool-acquire failure" >&2
        return 1
      }
      ;;
    *)
      echo "$label: unknown response validation $validation" >&2
      return 1
      ;;
  esac

  if c_parallax_required; then
    local trace_filter='any(.trace.spans[]?; .service == "inventory" and .name == "inventory.reserve")
      and any(.trace.spans[]?; .service == "inventory" and .name == "postgres.query" and
        ((try (.attributes | fromjson) catch {})["db.system.name"] == "postgresql"))'
    c_wait_for_trace "$trace_id" "$label trace" \
      "{ trace(traceId: \"$trace_id\") { spans { service name attributes } } }" \
      "$trace_filter"
  fi
}

echo "A25 Postgres: normal reserve"
request "normal" "${RUN_ID}-normal" WIDGET-1 200 reserved
echo

echo "A25 Postgres: slow query"
request "pg_sleep 400ms" "${RUN_ID}-slow" WIDGET-2 200 reserved --data-urlencode 'slow=400'
echo

echo "A25 Postgres: DB N+1"
request "db_n1=12" "${RUN_ID}-db-n1" GADGET-1 200 reserved --data-urlencode 'db_n1=12'
echo

echo "A25 Postgres: pool exhaustion"
pids=()
for i in $(seq 1 10); do
  reservation_id="${RUN_ID}-hold-$i"
  body="$HOLD_DIR/$i.body"
  code_file="$HOLD_DIR/$i.code"
  traceparent="$(c_traceparent_for "$(c_new_trace_id)")"
  (
    if code="$(curl --max-time 12 -sS --get "$BASE/reserve" \
      --data-urlencode "tenant_id=$TENANT_ID" \
      --data-urlencode "reservation_id=$reservation_id" \
      --data-urlencode 'sku=WIDGET-2' \
      --data-urlencode 'quantity=1' \
      --data-urlencode "checkout_request_id=$CHECKOUT_REQUEST_ID" \
      --data-urlencode "checkout_lease_token=$CHECKOUT_LEASE_TOKEN" \
      --data-urlencode 'hold_ms=4000' \
      -H "traceparent: $traceparent" -o "$body" -w '%{http_code}')"; then
      printf '%s' "$code" >"$code_file"
    else
      printf '000' >"$code_file"
      exit 1
    fi
  ) &
  pids+=("$!")
done

sleep "${A25_POOL_START_DELAY_SECONDS:-0.5}"
POOL_BODY="$WORK_DIR/pool.body"
POOL_TRACE_ID="$(c_new_trace_id)"
if pool_code="$(curl --max-time 8 -sS --get "$BASE/reserve" \
  --data-urlencode "tenant_id=$TENANT_ID" \
  --data-urlencode "reservation_id=${RUN_ID}-pool-probe" \
  --data-urlencode 'sku=WIDGET-1' \
  --data-urlencode 'quantity=1' \
  --data-urlencode "checkout_request_id=$CHECKOUT_REQUEST_ID" \
  --data-urlencode "checkout_lease_token=$CHECKOUT_LEASE_TOKEN" \
  -H "traceparent: $(c_traceparent_for "$POOL_TRACE_ID")" \
  -o "$POOL_BODY" -w '%{http_code}')"; then
  :
else
  pool_code=000
fi
printf "%-20s [%s] " "pool pressure" "$pool_code"
cat "$POOL_BODY"
printf '\n'

for pid in "${pids[@]}"; do
  wait "$pid"
done

if [[ "$pool_code" != "503" ]]; then
  echo "expected pool pressure request to return 503, got $pool_code" >&2
  cat "$POOL_BODY" >&2
  exit 1
fi
jq -e --arg reservation "${RUN_ID}-pool-probe" '
  .error == "inventory_unavailable" and .reservation_id == $reservation
' "$POOL_BODY" >/dev/null || {
  echo "pool pressure response did not prove inventory_unavailable" >&2
  cat "$POOL_BODY" >&2
  exit 1
}

for i in $(seq 1 10); do
  code="$(<"$HOLD_DIR/$i.code")"
  body="$HOLD_DIR/$i.body"
  case "$code" in
    200)
      location_id="$(jq -er '.location_id | strings | select(length > 0)' "$body")"
      release_reservation "${RUN_ID}-hold-$i" WIDGET-2 1 "$location_id"
      ;;
    503)
      jq -e '.error == "inventory_unavailable" or .error == "reservation_rejected"' "$body" >/dev/null || {
        echo "hold-$i: unexpected 503 response" >&2
        cat "$body" >&2
        exit 1
      }
      ;;
    *)
      echo "hold-$i: expected HTTP 200 or 503, got $code" >&2
      cat "$body" >&2
      exit 1
      ;;
  esac
done

if c_parallax_required; then
  c_wait_for_trace "$POOL_TRACE_ID" "pool pressure trace" \
    "{ trace(traceId: \"$POOL_TRACE_ID\") { spans { service name attributes } } }" \
    'any(.trace.spans[]?; .service == "inventory" and .name == "inventory.reserve")'
else
  echo "A25 Parallax trace assertions SKIPPED by SCENARIO_PARALLAX_MODE=skip"
fi

echo "A25 Postgres assertions PASS"

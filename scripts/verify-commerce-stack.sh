#!/usr/bin/env bash
set -Eeuo pipefail

# Fail-closed runtime proof. By default this script owns a uniquely named,
# disposable Compose project and removes its volumes. Set
# VERIFY_MANAGE_STACK=0 to inspect an already-running stack without Docker
# lifecycle changes.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/parallax-commerce-verify.XXXXXX")"
MANAGE_STACK="${VERIFY_MANAGE_STACK:-1}"
KEEP_STACK="${VERIFY_KEEP_STACK:-0}"
EXTERNAL_ALLOW_MUTATION="${VERIFY_EXTERNAL_ALLOW_MUTATION:-0}"
STACK_TIMEOUT="${VERIFY_STACK_TIMEOUT_SECONDS:-1800}"
CLEANUP_TIMEOUT="${VERIFY_CLEANUP_TIMEOUT_SECONDS:-120}"
CLEANUP_STOP_TIMEOUT="${VERIFY_CLEANUP_STOP_TIMEOUT_SECONDS:-20}"
HTTP_TIMEOUT="${VERIFY_HTTP_TIMEOUT_SECONDS:-20}"
FAILURE_HTTP_TIMEOUT="${VERIFY_FAILURE_HTTP_TIMEOUT_SECONDS:-3}"
ASYNC_TIMEOUT="${VERIFY_ASYNC_TIMEOUT_SECONDS:-60}"
FAILURE_PROBE_TIMEOUT="${VERIFY_FAILURE_PROBE_TIMEOUT_SECONDS:-30}"
DOCKER_PROBE_TIMEOUT="${VERIFY_DOCKER_PROBE_TIMEOUT_SECONDS:-10}"
PG_STATEMENT_TIMEOUT_MS="${VERIFY_PG_STATEMENT_TIMEOUT_MS:-10000}"
PG_LOCK_TIMEOUT_MS="${VERIFY_PG_LOCK_TIMEOUT_MS:-5000}"
REDIS_CONNECT_TIMEOUT="${VERIFY_REDIS_CONNECT_TIMEOUT_SECONDS:-5}"
COMPOSE_FILE="${VERIFY_COMPOSE_FILE:-$ROOT/deploy/docker-compose.yml}"
PARALLAX_API_URL="${VERIFY_PARALLAX_API_URL:-${PARALLAX_API_URL:-http://127.0.0.1:4000}}"
PARALLAX_CACHE_TELEMETRY="${VERIFY_PARALLAX_CACHE_TELEMETRY:-required}"
PROJECT=""
STACK_STARTED=0
COMPOSE=()
FLAG_CONFIG=""

fail() {
  echo "commerce stack verification failed: $*" >&2
  exit 1
}

terminate_process_tree() {
  local pid="$1" signal="$2" child
  while IFS= read -r child; do
    [[ -n "$child" ]] || continue
    terminate_process_tree "$child" "$signal"
  done < <(pgrep -P "$pid" 2>/dev/null || true)
  kill "-$signal" "$pid" >/dev/null 2>&1 || true
}

bounded_process_cleanup() {
  local pid="$1" deadline=$((SECONDS + 3))
  terminate_process_tree "$pid" TERM
  while kill -0 "$pid" >/dev/null 2>&1 && (( SECONDS < deadline )); do
    sleep 1
  done
  if kill -0 "$pid" >/dev/null 2>&1; then
    terminate_process_tree "$pid" KILL
  fi
  wait "$pid" >/dev/null 2>&1 || true
}

bounded_compose_down() {
  local log="$TMP_DIR/compose-down.log" pid deadline
  DOCKER_CLIENT_TIMEOUT="$CLEANUP_TIMEOUT" COMPOSE_HTTP_TIMEOUT="$CLEANUP_TIMEOUT" \
    "${COMPOSE[@]}" down --timeout "$CLEANUP_STOP_TIMEOUT" -v --remove-orphans >"$log" 2>&1 &
  pid=$!
  deadline=$((SECONDS + CLEANUP_TIMEOUT))
  while kill -0 "$pid" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      bounded_process_cleanup "$pid"
      echo "compose cleanup exceeded ${CLEANUP_TIMEOUT}s" >&2
      tail -n 80 "$log" >&2 || true
      return 1
    fi
    sleep 1
  done
  if ! wait "$pid"; then
    tail -n 80 "$log" >&2 || true
    return 1
  fi
}

cleanup() {
  local status=$?
  trap - EXIT
  if [[ "$MANAGE_STACK" == 1 && "$STACK_STARTED" == 1 ]]; then
    if [[ "$KEEP_STACK" == 1 ]]; then
      # Retained diagnostics must not retain credentials or payment requests.
      rm -f "$TMP_DIR"/*.netrc "$TMP_DIR"/checkout-request.json \
        "$TMP_DIR"/payment-*.json "$TMP_DIR"/flagd*.json 2>/dev/null || true
      echo "disposable stack kept: project=$PROJECT"
      echo "cleanup: ${COMPOSE[*]} down -v --remove-orphans"
      echo "temporary verifier files: $TMP_DIR"
    else
      echo "cleanup: removing disposable project $PROJECT"
      if ! bounded_compose_down; then
        echo "cleanup failed for disposable project $PROJECT" >&2
        tail -n 80 "$TMP_DIR/compose-down.log" >&2 || true
        [[ "$status" -eq 0 ]] && status=1
      fi
      rm -rf "$TMP_DIR"
    fi
  else
    rm -rf "$TMP_DIR"
  fi
  exit "$status"
}
trap cleanup EXIT

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is missing: $1"
}

require_positive_integer() {
  local name="$1" value="$2"
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "$name must be a positive integer: $value"
}

require_port() {
  local name="$1" value="$2"
  require_positive_integer "$name" "$value"
  (( value <= 65535 )) || fail "$name must be <= 65535: $value"
}

require_switch() {
  local name="$1" value="$2"
  [[ "$value" == 0 || "$value" == 1 ]] || fail "$name must be 0 or 1: $value"
}

require_positive_integer VERIFY_STACK_TIMEOUT_SECONDS "$STACK_TIMEOUT"
require_positive_integer VERIFY_CLEANUP_TIMEOUT_SECONDS "$CLEANUP_TIMEOUT"
require_positive_integer VERIFY_CLEANUP_STOP_TIMEOUT_SECONDS "$CLEANUP_STOP_TIMEOUT"
require_positive_integer VERIFY_HTTP_TIMEOUT_SECONDS "$HTTP_TIMEOUT"
require_positive_integer VERIFY_FAILURE_HTTP_TIMEOUT_SECONDS "$FAILURE_HTTP_TIMEOUT"
require_positive_integer VERIFY_ASYNC_TIMEOUT_SECONDS "$ASYNC_TIMEOUT"
require_positive_integer VERIFY_FAILURE_PROBE_TIMEOUT_SECONDS "$FAILURE_PROBE_TIMEOUT"
require_positive_integer VERIFY_DOCKER_PROBE_TIMEOUT_SECONDS "$DOCKER_PROBE_TIMEOUT"
require_positive_integer VERIFY_PG_STATEMENT_TIMEOUT_MS "$PG_STATEMENT_TIMEOUT_MS"
require_positive_integer VERIFY_PG_LOCK_TIMEOUT_MS "$PG_LOCK_TIMEOUT_MS"
require_positive_integer VERIFY_REDIS_CONNECT_TIMEOUT_SECONDS "$REDIS_CONNECT_TIMEOUT"
require_switch VERIFY_MANAGE_STACK "$MANAGE_STACK"
require_switch VERIFY_KEEP_STACK "$KEEP_STACK"
require_switch VERIFY_EXTERNAL_ALLOW_MUTATION "$EXTERNAL_ALLOW_MUTATION"
case "$PARALLAX_CACHE_TELEMETRY" in
  auto|skip|required) ;;
  *) fail "VERIFY_PARALLAX_CACHE_TELEMETRY must be auto, skip, or required: $PARALLAX_CACHE_TELEMETRY" ;;
esac
for command_name in curl jq base64 grep cmp buf pgrep; do require_command "$command_name"; done
if [[ "$MANAGE_STACK" == 0 ]]; then
  for command_name in psql redis-cli; do require_command "$command_name"; done
fi

TENANT_ID="${VERIFY_TENANT_ID:-tenant-acme}"
CUSTOMER_ID="${VERIFY_CUSTOMER_ID:-customer-acme-ava}"
SKU="${VERIFY_SKU:-WIDGET-1}"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
[[ "$TENANT_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "VERIFY_TENANT_ID contains unsafe characters"
[[ "$CUSTOMER_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "VERIFY_CUSTOMER_ID contains unsafe characters"
[[ "$SKU" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "VERIFY_SKU contains unsafe characters"
PRIMARY_SESSION_ID="session-${RUN_ID}"
SESSION_ID="$PRIMARY_SESSION_ID"
[[ "$PRIMARY_SESSION_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "generated session id is unsafe"

POSTGRES_URL="${POSTGRES_URL:-}"
POSTGRES_HOST="${POSTGRES_HOST:-${PGHOST:-127.0.0.1}}"
POSTGRES_PORT="${POSTGRES_PORT:-${PGPORT:-5432}}"
POSTGRES_DB="${POSTGRES_DB:-${PGDATABASE:-playground}}"
POSTGRES_USER="${POSTGRES_USER:-${PGUSER:-postgres}}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-playground}}"
PGCONNECT_TIMEOUT="${PGCONNECT_TIMEOUT:-5}"
require_positive_integer PGCONNECT_TIMEOUT "$PGCONNECT_TIMEOUT"
export DOCKER_CLIENT_TIMEOUT="$STACK_TIMEOUT" COMPOSE_HTTP_TIMEOUT="$STACK_TIMEOUT"
REDIS_URL="${REDIS_URL:-}"
REDIS_HOST="${REDIS_HOST:-127.0.0.1}"
REDIS_PORT="${REDIS_PORT:-6379}"
REDIS_DB="${REDIS_DB:-0}"
REDIS_PASSWORD="${REDIS_PASSWORD:-}"
RABBITMQ_MANAGEMENT_URL="${RABBITMQ_MANAGEMENT_URL:-http://127.0.0.1:15672}"
RABBITMQ_USER="${RABBITMQ_USER:-${RABBITMQ_USERNAME:-playground}}"
RABBITMQ_PASSWORD="${RABBITMQ_PASSWORD:-playground}"
RABBITMQ_VHOST_PATH="${RABBITMQ_VHOST_PATH:-%2F}"
CLICKHOUSE_URL="${CLICKHOUSE_URL:-http://127.0.0.1:8123}"
CLICKHOUSE_USER="${CLICKHOUSE_USER:-default}"
CLICKHOUSE_PASSWORD="${CLICKHOUSE_PASSWORD:-}"
CHECKOUT_URL="${CHECKOUT_URL:-http://127.0.0.1:8088}"
PAYMENT_GRPC_URL="${PAYMENT_GRPC_URL:-http://127.0.0.1:9090}"
CATALOG_URL="${CATALOG_URL:-http://127.0.0.1:8080}"
CATALOG_GRAPHQL_URL="${CATALOG_GRAPHQL_URL:-${CATALOG_URL%/}/graphql}"
FULFILLMENT_URL="${FULFILLMENT_URL:-http://127.0.0.1:8093}"
FULFILLMENT_INTERNAL_TOKEN="${FULFILLMENT_INTERNAL_TOKEN:-fulfillment-internal:research-secret}"
WEB_URL="${WEB_URL:-http://127.0.0.1:3000}"
FLAGD_HEALTH_URL="${FLAGD_HEALTH_URL:-http://127.0.0.1:8014/healthz}"
STOREFRONT_GRAPHQL_URL="${STOREFRONT_GRAPHQL_URL:-http://127.0.0.1:8094/graphql}"
FULFILLMENT_QUEUE="${VERIFY_FULFILLMENT_QUEUE:-fulfillment.orders}"
ANALYTICS_QUEUE="${VERIFY_ANALYTICS_QUEUE:-analytics.events}"
EXPECTED_CHECKOUT_VARIANT="${VERIFY_EXPECTED_CHECKOUT_VARIANT:-orchestrated}"
[[ "$EXPECTED_CHECKOUT_VARIANT" == control || "$EXPECTED_CHECKOUT_VARIANT" == orchestrated ]] || \
  fail "VERIFY_EXPECTED_CHECKOUT_VARIANT must be control or orchestrated"
[[ "$FULFILLMENT_QUEUE" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
  fail "VERIFY_FULFILLMENT_QUEUE contains unsafe characters"
[[ "$ANALYTICS_QUEUE" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
  fail "VERIFY_ANALYTICS_QUEUE contains unsafe characters"
[[ -n "$FULFILLMENT_INTERNAL_TOKEN" && "$FULFILLMENT_INTERNAL_TOKEN" != *$'\n'* \
  && "$FULFILLMENT_INTERNAL_TOKEN" != *$'\r'* ]] || \
  fail "FULFILLMENT_INTERNAL_TOKEN must be a non-empty single-line value"
FULFILLMENT_AUTH_HEADERS=(
  -H "authorization: Bearer $FULFILLMENT_INTERNAL_TOKEN"
  -H "x-tenant-id: $TENANT_ID"
)

url_host() {
  [[ "$1" =~ ^https?://[^/@[:space:]]+(:[0-9]+)?(/[^[:space:]]*)?$ ]] || \
    fail "URL must be an HTTP(S) URL without embedded credentials: $1"
  local authority="${1#*://}"
  authority="${authority%%/*}"
  authority="${authority##*@}"
  authority="${authority%%:*}"
  [[ -n "$authority" ]] || fail "URL must include a host"
  printf '%s' "$authority"
}

validate_vhost_path() {
  [[ "$1" =~ ^([A-Za-z0-9._~-]|%[0-9A-Fa-f]{2})+$ ]] || \
    fail "RABBITMQ_VHOST_PATH must be URL-encoded and contain no raw slash: $1"
}

write_netrc() {
  local path="$1" url="$2" user="$3" password="$4" host
  host="$(url_host "$url")"
  [[ "$user" != *[[:space:]]* && "$password" != *[[:space:]]* ]] || \
    fail "management credentials cannot contain whitespace"
  printf 'machine %s login %s password %s\n' "$host" "$user" "$password" >"$path"
  chmod 600 "$path"
}

if [[ "$MANAGE_STACK" == 1 ]]; then
  require_command docker
  require_command bun
  [[ -z "${VERIFY_COMPOSE_FILE:-}" ]] || \
    fail "VERIFY_COMPOSE_FILE is external-only; managed mode owns the canonical Compose file"
  [[ -f "$COMPOSE_FILE" ]] || fail "Compose file is missing: $COMPOSE_FILE"
  PROJECT="telemetry-playground-verification-$(date +%s)-$$"

  [[ -z "$POSTGRES_URL" ]] || fail "POSTGRES_URL is external-only in managed mode"
  [[ -z "$REDIS_URL" ]] || fail "REDIS_URL is external-only in managed mode"
  [[ "$POSTGRES_DB" == playground && "$POSTGRES_USER" == postgres ]] || \
    fail "managed mode requires the canonical PostgreSQL database/user: playground/postgres"
  [[ "$REDIS_DB" == 0 ]] || fail "managed mode requires Redis database 0"
  [[ "$RABBITMQ_USER" == playground && "$RABBITMQ_VHOST_PATH" == %2F ]] || \
    fail "managed mode requires RabbitMQ user playground and encoded vhost %2F"
  [[ "$CLICKHOUSE_USER" == default ]] || fail "managed mode requires ClickHouse user default"
  [[ "$TENANT_ID" == tenant-acme && "$CUSTOMER_ID" == customer-acme-ava && "$SKU" == WIDGET-1 ]] || \
    fail "managed mode verifies only the canonical tenant/customer/SKU seed"
  [[ "$FULFILLMENT_QUEUE" == fulfillment.orders ]] || \
    fail "managed mode requires fulfillment.orders queue"
  [[ "$ANALYTICS_QUEUE" == analytics.events ]] || \
    fail "managed mode requires analytics.events queue"
  validate_vhost_path "$RABBITMQ_VHOST_PATH"

  POSTGRES_HOST_PORT="${VERIFY_POSTGRES_HOST_PORT:-25432}"
  RABBITMQ_AMQP_HOST_PORT="${VERIFY_RABBITMQ_AMQP_HOST_PORT:-25672}"
  RABBITMQ_MANAGEMENT_HOST_PORT="${VERIFY_RABBITMQ_MANAGEMENT_HOST_PORT:-35672}"
  REDIS_HOST_PORT="${VERIFY_REDIS_HOST_PORT:-26379}"
  CLICKHOUSE_HTTP_HOST_PORT="${VERIFY_CLICKHOUSE_HTTP_HOST_PORT:-28123}"
  CLICKHOUSE_NATIVE_HOST_PORT="${VERIFY_CLICKHOUSE_NATIVE_HOST_PORT:-29000}"
  FLAGD_RPC_HOST_PORT="${VERIFY_FLAGD_RPC_HOST_PORT:-28013}"
  FLAGD_HEALTH_HOST_PORT="${VERIFY_FLAGD_HEALTH_HOST_PORT:-28014}"
  CHECKOUT_HOST_PORT="${VERIFY_CHECKOUT_HOST_PORT:-18088}"
  PAYMENT_HOST_PORT="${VERIFY_PAYMENT_HOST_PORT:-18091}"
  INVENTORY_HOST_PORT="${VERIFY_INVENTORY_HOST_PORT:-18089}"
  RECOMMENDATION_HOST_PORT="${VERIFY_RECOMMENDATION_HOST_PORT:-18090}"
  ORDERS_HOST_PORT="${VERIFY_ORDERS_HOST_PORT:-18092}"
  FULFILLMENT_HOST_PORT="${VERIFY_FULFILLMENT_HOST_PORT:-18093}"
  STOREFRONT_HOST_PORT="${VERIFY_STOREFRONT_HOST_PORT:-18094}"
  CATALOG_HOST_PORT="${VERIFY_CATALOG_HOST_PORT:-18080}"
  WEB_HOST_PORT="${VERIFY_WEB_HOST_PORT:-15173}"
  for port_spec in \
    POSTGRES_HOST_PORT:$POSTGRES_HOST_PORT RABBITMQ_AMQP_HOST_PORT:$RABBITMQ_AMQP_HOST_PORT \
    RABBITMQ_MANAGEMENT_HOST_PORT:$RABBITMQ_MANAGEMENT_HOST_PORT REDIS_HOST_PORT:$REDIS_HOST_PORT \
    CLICKHOUSE_HTTP_HOST_PORT:$CLICKHOUSE_HTTP_HOST_PORT CLICKHOUSE_NATIVE_HOST_PORT:$CLICKHOUSE_NATIVE_HOST_PORT \
    FLAGD_RPC_HOST_PORT:$FLAGD_RPC_HOST_PORT FLAGD_HEALTH_HOST_PORT:$FLAGD_HEALTH_HOST_PORT \
    CHECKOUT_HOST_PORT:$CHECKOUT_HOST_PORT PAYMENT_HOST_PORT:$PAYMENT_HOST_PORT \
    INVENTORY_HOST_PORT:$INVENTORY_HOST_PORT \
    RECOMMENDATION_HOST_PORT:$RECOMMENDATION_HOST_PORT ORDERS_HOST_PORT:$ORDERS_HOST_PORT \
    FULFILLMENT_HOST_PORT:$FULFILLMENT_HOST_PORT STOREFRONT_HOST_PORT:$STOREFRONT_HOST_PORT \
    CATALOG_HOST_PORT:$CATALOG_HOST_PORT WEB_HOST_PORT:$WEB_HOST_PORT; do
    require_port "${port_spec%%:*}" "${port_spec#*:}"
  done

  export POSTGRES_PASSWORD RABBITMQ_PASSWORD CLICKHOUSE_PASSWORD
  POSTGRES_HOST=127.0.0.1; POSTGRES_PORT="$POSTGRES_HOST_PORT"
  REDIS_HOST=127.0.0.1; REDIS_PORT="$REDIS_HOST_PORT"
  RABBITMQ_MANAGEMENT_URL="http://127.0.0.1:$RABBITMQ_MANAGEMENT_HOST_PORT"
  CLICKHOUSE_URL="http://127.0.0.1:$CLICKHOUSE_HTTP_HOST_PORT"
  CHECKOUT_URL="http://127.0.0.1:$CHECKOUT_HOST_PORT"
  PAYMENT_GRPC_URL="http://127.0.0.1:$PAYMENT_HOST_PORT"
  CATALOG_URL="http://127.0.0.1:$CATALOG_HOST_PORT"
  CATALOG_GRAPHQL_URL="$CATALOG_URL/graphql"
  STOREFRONT_GRAPHQL_URL="http://127.0.0.1:$STOREFRONT_HOST_PORT/graphql"
  FULFILLMENT_URL="http://127.0.0.1:$FULFILLMENT_HOST_PORT"
  WEB_URL="http://127.0.0.1:$WEB_HOST_PORT"
  FLAGD_HEALTH_URL="http://127.0.0.1:$FLAGD_HEALTH_HOST_PORT/healthz"
  EXPECTED_CHECKOUT_VARIANT="${VERIFY_INITIAL_CHECKOUT_VARIANT:-control}"
  [[ "$EXPECTED_CHECKOUT_VARIANT" == control || "$EXPECTED_CHECKOUT_VARIANT" == orchestrated ]] || \
    fail "VERIFY_INITIAL_CHECKOUT_VARIANT must be control or orchestrated"

  FLAG_CONFIG="$TMP_DIR/flagd.json"
  jq -e '.flags.checkoutFlow.variants.control and .flags.checkoutFlow.variants.orchestrated' \
    "$ROOT/flags/flagd.json" >/dev/null || fail "checkoutFlow variants are missing"
  jq --arg variant "$EXPECTED_CHECKOUT_VARIANT" \
    '.flags.checkoutFlow.defaultVariant = $variant' "$ROOT/flags/flagd.json" >"$FLAG_CONFIG"
  OVERLAY="$TMP_DIR/compose.override.yml"
  cat >"$OVERLAY" <<EOF
services:
  postgres:
    ports: !override ["${POSTGRES_HOST_PORT}:5432"]
  rabbitmq:
    ports: !override ["${RABBITMQ_AMQP_HOST_PORT}:5672", "${RABBITMQ_MANAGEMENT_HOST_PORT}:15672"]
  redis:
    ports: !override ["${REDIS_HOST_PORT}:6379"]
  clickhouse:
    ports: !override ["${CLICKHOUSE_HTTP_HOST_PORT}:8123", "${CLICKHOUSE_NATIVE_HOST_PORT}:9000"]
  flagd:
    ports: !override ["${FLAGD_RPC_HOST_PORT}:8013", "${FLAGD_HEALTH_HOST_PORT}:8014"]
    volumes: !override
      - "${FLAG_CONFIG}:/etc/flagd/flagd.json:ro"
      - "flagd_health_tools:/health-tools:ro"
  checkout:
    ports: !override ["${CHECKOUT_HOST_PORT}:8088"]
  payment:
    ports: !override ["${PAYMENT_HOST_PORT}:9090"]
  inventory:
    ports: !override ["${INVENTORY_HOST_PORT}:8089"]
  recommendation:
    ports: !override ["${RECOMMENDATION_HOST_PORT}:8090"]
  orders:
    ports: !override ["${ORDERS_HOST_PORT}:8092"]
  fulfillment:
    ports: !override ["${FULFILLMENT_HOST_PORT}:8080"]
  storefront:
    ports: !override ["${STOREFRONT_HOST_PORT}:8094"]
    environment:
      WEB_ORIGIN: "http://127.0.0.1:${WEB_HOST_PORT},http://localhost:${WEB_HOST_PORT}"
  catalog:
    ports: !override ["${CATALOG_HOST_PORT}:8080"]
  web:
    build:
      args:
        VITE_STOREFRONT_URL: "/__storefront/graphql"
    ports: !override ["${WEB_HOST_PORT}:3000"]
EOF
  COMPOSE=(docker compose -p "$PROJECT" -f "$COMPOSE_FILE" -f "$OVERLAY")
  STACK_STARTED=1
  "${COMPOSE[@]}" config --quiet || fail "Compose configuration is invalid"
else
  [[ -z "${VERIFY_COMPOSE_PROJECT:-}" ]] || fail "VERIFY_COMPOSE_PROJECT is forbidden in external mode"
  [[ "$EXTERNAL_ALLOW_MUTATION" == 1 ]] || \
    fail "external mode mutates durable commerce state; set VERIFY_EXTERNAL_ALLOW_MUTATION=1 explicitly"
  [[ -n "${VERIFY_TENANT_ID:-}" && -n "${VERIFY_CUSTOMER_ID:-}" && -n "${VERIFY_SKU:-}" ]] || \
    fail "external mode requires explicit isolated VERIFY_TENANT_ID, VERIFY_CUSTOMER_ID, and VERIFY_SKU"
  [[ "$TENANT_ID" != tenant-acme || "$CUSTOMER_ID" != customer-acme-ava ]] || \
    fail "external mode requires a tenant/customer fixture isolated from the canonical verifier"
  validate_vhost_path "$RABBITMQ_VHOST_PATH"
fi

export PGCONNECT_TIMEOUT
if [[ "$MANAGE_STACK" == 1 ]]; then
  PSQL=("${COMPOSE[@]}" exec -T -e "PGCONNECT_TIMEOUT=$PGCONNECT_TIMEOUT" postgres psql --no-psqlrc --set=ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB")
elif [[ -n "$POSTGRES_URL" ]]; then
  PSQL=(psql --no-psqlrc --set=ON_ERROR_STOP=1 "$POSTGRES_URL")
else
  export PGHOST="$POSTGRES_HOST" PGPORT="$POSTGRES_PORT" PGDATABASE="$POSTGRES_DB"
  export PGUSER="$POSTGRES_USER" PGPASSWORD="$POSTGRES_PASSWORD" PGCONNECT_TIMEOUT
  PSQL=(psql --no-psqlrc --set=ON_ERROR_STOP=1)
fi

psql_query() {
  {
    printf "SET lock_timeout = '%sms';\n" "$PG_LOCK_TIMEOUT_MS"
    printf "SET statement_timeout = '%sms';\n" "$PG_STATEMENT_TIMEOUT_MS"
    printf '%s\n' "$1"
  } | "${PSQL[@]}" -Atq
}

redis_cli() {
  if [[ "$MANAGE_STACK" == 1 ]]; then
    "${COMPOSE[@]}" exec -T redis redis-cli -t "$REDIS_CONNECT_TIMEOUT" --raw "$@"
  elif [[ -n "$REDIS_URL" ]]; then
    REDISCLI_AUTH="$REDIS_PASSWORD" redis-cli -t "$REDIS_CONNECT_TIMEOUT" --raw -u "$REDIS_URL" "$@"
  else
    REDISCLI_AUTH="$REDIS_PASSWORD" redis-cli -t "$REDIS_CONNECT_TIMEOUT" --raw -h "$REDIS_HOST" -p "$REDIS_PORT" -n "$REDIS_DB" "$@"
  fi
}

RABBIT_NETRC="$TMP_DIR/rabbit.netrc"
CLICKHOUSE_NETRC="$TMP_DIR/clickhouse.netrc"
write_netrc "$RABBIT_NETRC" "$RABBITMQ_MANAGEMENT_URL" "$RABBITMQ_USER" "$RABBITMQ_PASSWORD"
write_netrc "$CLICKHOUSE_NETRC" "$CLICKHOUSE_URL" "$CLICKHOUSE_USER" "$CLICKHOUSE_PASSWORD"
rabbit_queue_json() {
  curl -fsS --max-time "$HTTP_TIMEOUT" --netrc-file "$RABBIT_NETRC" \
    "${RABBITMQ_MANAGEMENT_URL%/}/api/queues/${RABBITMQ_VHOST_PATH}/$1"
}
rabbit_queue_json_bounded() {
  curl -fsS --max-time "$FAILURE_HTTP_TIMEOUT" --netrc-file "$RABBIT_NETRC" \
    "${RABBITMQ_MANAGEMENT_URL%/}/api/queues/${RABBITMQ_VHOST_PATH}/$1"
}
rabbit_queue_bindings_json() {
  curl -fsS --max-time "$HTTP_TIMEOUT" --netrc-file "$RABBIT_NETRC" \
    "${RABBITMQ_MANAGEMENT_URL%/}/api/queues/${RABBITMQ_VHOST_PATH}/$1/bindings"
}
clickhouse_query() {
  curl -fsS --max-time "$HTTP_TIMEOUT" --netrc-file "$CLICKHOUSE_NETRC" \
    --get --data-urlencode "query=$1 FORMAT TabSeparatedRaw" "${CLICKHOUSE_URL%/}/"
}

parallax_graphql_url() {
  if [[ "$PARALLAX_API_URL" == */graphql ]]; then
    printf '%s' "$PARALLAX_API_URL"
  else
    printf '%s/graphql' "${PARALLAX_API_URL%/}"
  fi
}

parallax_loopback_url() {
  local url="$1" authority
  [[ "$url" =~ ^https?://([^/?#]+)([/?#].*)?$ ]] || return 1
  authority="${BASH_REMATCH[1]}"
  [[ "$authority" =~ ^(localhost|127[.]0[.]0[.]1)(:[0-9]{1,5})?$ ||
    "$authority" =~ ^\[::1\](:[0-9]{1,5})?$ ]]
}

parallax_trace() {
  local trace_id="$1" authorize="${2:-1}" query
  local -a headers=(-H 'content-type: application/json')
  query="{ trace(traceId: \"$trace_id\") { spans { name service attributes } } }"
  if [[ "$authorize" == 1 && -n "${PARALLAX_API_TOKEN:-}" ]]; then
    headers+=(-H "authorization: Bearer $PARALLAX_API_TOKEN")
  fi
  curl -fsS --max-time "$HTTP_TIMEOUT" "${headers[@]}" \
    --data "$(jq -cn --arg query "$query" '{query:$query}')" \
    "$(parallax_graphql_url)"
}

parallax_trace_has_cache_result() {
  local trace_json="$1" service="$2" attribute="$3" result="$4"
  jq -e --arg service "$service" --arg attribute "$attribute" --arg result "$result" '
    any(.data.trace.spans[]?;
      .service == $service
      and (((.attributes // "")
            | if type == "string" then (try fromjson catch {}) else . end)
          [$attribute] == $result))
  ' <<<"$trace_json" >/dev/null 2>&1
}

assert_cache_telemetry() {
  [[ "$PARALLAX_CACHE_TELEMETRY" == skip ]] && return 0
  local graphql_url probe_response cold_trace warm_trace deadline trace_authorize=1
  local -a headers=(-H 'content-type: application/json')
  graphql_url="$(parallax_graphql_url)"
  probe_response="$TMP_DIR/parallax-probe.json"
  if [[ -z "${PARALLAX_API_TOKEN:-}" ]]; then
    if ! parallax_loopback_url "$PARALLAX_API_URL"; then
      if [[ "$PARALLAX_CACHE_TELEMETRY" == required ]]; then
        fail "PARALLAX_API_TOKEN is required for external Redis cache telemetry proof"
      fi
      echo "Redis cache telemetry proof skipped: PARALLAX_API_TOKEN is unavailable for external Parallax"
      return 0
    fi
    # Loopback Parallax may intentionally run without authentication. Keep this
    # request unauthenticated; remote endpoints never reach this branch.
    trace_authorize=0
  else
    headers+=(-H "authorization: Bearer $PARALLAX_API_TOKEN")
  fi
  if ! curl -fsS --max-time 2 -o "$probe_response" \
      "${headers[@]}" --data '{"query":"{ __typename }"}' \
      "$graphql_url" >/dev/null 2>&1; then
    if [[ "$PARALLAX_CACHE_TELEMETRY" == required ]]; then
      fail "Parallax GraphQL is required for Redis cache telemetry proof: $graphql_url"
    fi
    echo "Redis cache telemetry proof skipped: Parallax GraphQL is unavailable"
    return 0
  fi
  if ! jq -e '((.errors // []) | length == 0) and .data.__typename == "Query"' \
      "$probe_response" >/dev/null 2>&1; then
    if [[ "$PARALLAX_CACHE_TELEMETRY" == required ]]; then
      fail "Parallax GraphQL probe returned an invalid or unauthenticated response: $graphql_url"
    fi
    echo "Redis cache telemetry proof skipped: Parallax GraphQL probe was invalid"
    return 0
  fi

  deadline=$((SECONDS + ASYNC_TIMEOUT))
  while (( SECONDS < deadline )); do
    cold_trace="$(parallax_trace "$CATALOG_COLD_TRACE_ID" "$trace_authorize" \
      2>"$TMP_DIR/parallax-catalog-cold.err" || true)"
    warm_trace="$(parallax_trace "$CATALOG_WARM_TRACE_ID" "$trace_authorize" \
      2>"$TMP_DIR/parallax-catalog-warm.err" || true)"
    if parallax_trace_has_cache_result "$cold_trace" catalog catalog.cache miss \
      && parallax_trace_has_cache_result "$warm_trace" catalog catalog.cache hit; then
      pricing_cold_trace="$(parallax_trace "$PRICING_COLD_TRACE_ID" "$trace_authorize" \
        2>"$TMP_DIR/parallax-pricing-cold.err" || true)"
      pricing_warm_trace="$(parallax_trace "$PRICING_WARM_TRACE_ID" "$trace_authorize" \
        2>"$TMP_DIR/parallax-pricing-warm.err" || true)"
      if parallax_trace_has_cache_result "$pricing_cold_trace" pricing cache.result miss \
        && parallax_trace_has_cache_result "$pricing_warm_trace" pricing cache.result hit; then
        echo "Redis cold-miss/warm-hit telemetry passed for catalog and pricing"
        return 0
      fi
    fi
    sleep 1
  done
  if [[ "$PARALLAX_CACHE_TELEMETRY" == required ]]; then
    fail "Redis cache telemetry did not show cold miss and warm hit within ${ASYNC_TIMEOUT}s"
  fi
  echo "Redis cache telemetry proof skipped: spans were not available within ${ASYNC_TIMEOUT}s"
}

run_bounded_seconds() {
  local label="$1" timeout_seconds="$2"; shift 2
  local log="$TMP_DIR/$label.log" pid deadline
  "$@" >"$log" 2>&1 & pid=$!
  deadline=$((SECONDS + timeout_seconds))
  while kill -0 "$pid" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      bounded_process_cleanup "$pid"
      echo "$label exceeded ${timeout_seconds}s" >&2
      tail -n 80 "$log" >&2 || true
      return 124
    fi
    sleep 1
  done
  if ! wait "$pid"; then
    echo "$label failed" >&2
    tail -n 80 "$log" >&2 || true
    return 1
  fi
}
run_bounded() {
  run_bounded_seconds "$1" "$STACK_TIMEOUT" "${@:2}"
}

docker_probe() {
  DOCKER_CLIENT_TIMEOUT="$DOCKER_PROBE_TIMEOUT" COMPOSE_HTTP_TIMEOUT="$DOCKER_PROBE_TIMEOUT" \
    docker "$@"
}
compose_logs() {
  DOCKER_CLIENT_TIMEOUT="$DOCKER_PROBE_TIMEOUT" COMPOSE_HTTP_TIMEOUT="$DOCKER_PROBE_TIMEOUT" \
    "${COMPOSE[@]}" logs --no-color --tail=80 "$1" >&2 || true
}
container_id() {
  local id
  id="$(DOCKER_CLIENT_TIMEOUT="$DOCKER_PROBE_TIMEOUT" COMPOSE_HTTP_TIMEOUT="$DOCKER_PROBE_TIMEOUT" \
    "${COMPOSE[@]}" ps --all -q "$1" 2>/dev/null | sed -n '1p' | tr -d '\r')"
  [[ "$id" =~ ^[0-9a-f]{12,64}$ ]] || return 1
  printf '%s' "$id"
}
wait_for_healthy() {
  local service="$1" timeout_seconds="${2:-$STACK_TIMEOUT}" id state health code deadline
  deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if id="$(container_id "$service")"; then
      state="$(docker_probe inspect --format '{{.State.Status}}' "$id" 2>/dev/null || true)"
      code="$(docker_probe inspect --format '{{.State.ExitCode}}' "$id" 2>/dev/null || true)"
      health="$(docker_probe inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "$id" 2>/dev/null || true)"
      [[ "$health" == healthy ]] && return 0
      if [[ "$state" == exited || "$state" == dead ]]; then
        echo "$service exited before becoming healthy (exit=$code)" >&2
        compose_logs "$service"
        return 1
      fi
    fi
    sleep 1
  done
  echo "$service did not become healthy within ${timeout_seconds}s" >&2
  compose_logs "$service"
  return 1
}
wait_for_completed() {
  local service="$1" timeout_seconds="${2:-$STACK_TIMEOUT}" id state code deadline
  deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if id="$(container_id "$service")"; then
      state="$(docker_probe inspect --format '{{.State.Status}}' "$id" 2>/dev/null || true)"
      code="$(docker_probe inspect --format '{{.State.ExitCode}}' "$id" 2>/dev/null || true)"
      if [[ "$state" == exited ]]; then
        [[ "$code" == 0 ]] && return 0
        echo "$service exited unsuccessfully (exit=$code)" >&2
        compose_logs "$service"
        return 1
      fi
    fi
    sleep 1
  done
  echo "$service did not complete within ${timeout_seconds}s" >&2
  compose_logs "$service"
  return 1
}

wait_for_container_state() {
  local service="$1" expected="$2" timeout_seconds="${3:-$FAILURE_PROBE_TIMEOUT}" id state deadline
  deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if id="$(container_id "$service")"; then
      state="$(docker_probe inspect --format '{{.State.Status}}' "$id" 2>/dev/null || true)"
      [[ "$state" == "$expected" ]] && return 0
    fi
    sleep 1
  done
  echo "$service did not reach state $expected within ${timeout_seconds}s" >&2
  return 1
}

run_bounded proto-lint buf lint "$ROOT/proto"
run_bounded proto-build buf build "$ROOT/proto"
echo "Buf lint and build passed"

if [[ "$MANAGE_STACK" == 1 ]]; then
  echo "starting disposable Compose project $PROJECT"
  case "${VERIFY_BUILD:-1}" in
    1) run_bounded compose-up "${COMPOSE[@]}" up -d --build ;;
    0) run_bounded compose-up "${COMPOSE[@]}" up -d ;;
    *) fail "VERIFY_BUILD must be 0 or 1" ;;
  esac
  for service in flagd-health-tools postgres-migrate clickhouse-init; do
    wait_for_completed "$service" || fail "initialization failed: $service"
  done
  for service in postgres rabbitmq redis clickhouse flagd pricing inventory recommendation notifications orders \
    catalog payment fulfillment checkout storefront web; do
    wait_for_healthy "$service" || fail "service is not ready: $service"
  done
  before_migrations="$(psql_query "SELECT string_agg(version || '=' || applied_at::text, ',' ORDER BY version) FROM public.schema_migrations;")"
  run_bounded migration-rerun "${COMPOSE[@]}" run --rm --no-deps postgres-migrate
  after_migrations="$(psql_query "SELECT string_agg(version || '=' || applied_at::text, ',' ORDER BY version) FROM public.schema_migrations;")"
  [[ "$before_migrations" == "$after_migrations" ]] || fail "migration rerun changed schema_migrations"
  echo "migration proof passed: clean apply plus identical rerun"
fi

# migrate.sh records full filenames. This is 18 rows: 001, three 002 files,
# 003-009, and 010-016. A stale count would fail on every clean database.
expected_migration_versions="001-commerce,002-catalog-price-change-events,002-durable-checkout,002-fulfillment-claim-leases,003-compensation-terminal-state,004-compensation-propagation,005-outbox-recovery,006-orders-consumer-inbox,007-fulfillment-claim-isolation,008-outbox-occurred-at,009-tenant-scoped-integrity,010-assortment-expansion,011-checkout-fencing-inventory-lifecycle,012-payment-pending-reconciliation,013-notification-delivery-durability,014-fulfillment-effects,015-checkout-generation-claims,016-outbox-propagation-contract"
actual_migration_versions="$(psql_query "SELECT COALESCE(string_agg(version, ',' ORDER BY version), '') FROM public.schema_migrations;")"
[[ "$actual_migration_versions" == "$expected_migration_versions" ]] || \
  fail "migration set drifted: expected $expected_migration_versions, got $actual_migration_versions"
schema_count="$(psql_query "SELECT count(*) FROM public.schema_migrations;" | tr -d '[:space:]')"
[[ "$schema_count" == 18 ]] || fail "expected eighteen migrations, got $schema_count"
seed_check="$(psql_query "
  SELECT (
    (SELECT count(*) FROM tenants WHERE id = '${TENANT_ID}' AND default_currency = 'USD' AND status = 'active') = 1
    AND (SELECT count(*) FROM customers WHERE tenant_id = '${TENANT_ID}' AND id = '${CUSTOMER_ID}' AND status = 'active') = 1
    AND (SELECT count(*) FROM products p JOIN categories c ON c.tenant_id = p.tenant_id AND c.id = p.category_id
         WHERE p.tenant_id = '${TENANT_ID}' AND p.id = 'prod-acme-widget' AND p.slug = 'everyday-widget'
           AND p.status = 'active' AND c.id = 'cat-acme-kitchen' AND c.slug = 'kitchen') = 1
    AND (SELECT count(*) FROM product_variants
         WHERE tenant_id = '${TENANT_ID}' AND id = 'var-acme-widget-1' AND sku = '${SKU}' AND status = 'active') = 1
    AND (SELECT count(*) FROM prices p JOIN product_variants v ON v.tenant_id = p.tenant_id AND v.id = p.variant_id
         WHERE p.tenant_id = '${TENANT_ID}' AND p.id = 'price-acme-widget-1-usd' AND v.id = 'var-acme-widget-1'
           AND v.sku = '${SKU}' AND p.currency = 'USD' AND p.amount_minor = 1999 AND p.is_default AND p.valid_to IS NULL) = 1
    AND (SELECT count(*) FROM inventory i
         WHERE i.tenant_id = '${TENANT_ID}' AND i.id = 'inventory-acme-widget-1-east'
           AND i.variant_id = 'var-acme-widget-1' AND i.location_id = 'loc-acme-east'
           AND i.on_hand_quantity = 100 AND i.reserved_quantity = 4 AND i.available_quantity = 96) = 1
    AND (SELECT count(*) FROM products
         WHERE tenant_id = '${TENANT_ID}' AND status = 'active') >= 24
    AND (SELECT count(*) FROM products
         WHERE tenant_id = '${TENANT_ID}' AND category_id = 'cat-acme-kitchen' AND status = 'active') >= 12
    AND (SELECT count(*) FROM products
         WHERE tenant_id = '${TENANT_ID}' AND category_id = 'cat-acme-electronics' AND status = 'active') >= 12
    AND (SELECT count(*)
         FROM products p
         JOIN product_variants v ON v.tenant_id = p.tenant_id AND v.product_id = p.id
         WHERE p.tenant_id = '${TENANT_ID}' AND p.status = 'active'
           AND v.status = 'active' AND v.sku LIKE 'ACME-DEMO-%') = 24
    AND (SELECT count(DISTINCT v.id)
         FROM product_variants v
         JOIN prices p ON p.tenant_id = v.tenant_id AND p.variant_id = v.id
         JOIN inventory i ON i.tenant_id = v.tenant_id AND i.variant_id = v.id
         JOIN products product ON product.tenant_id = v.tenant_id AND product.id = v.product_id
         JOIN reviews r ON r.tenant_id = v.tenant_id AND r.product_id = v.product_id
         WHERE v.tenant_id = '${TENANT_ID}' AND v.status = 'active'
           AND v.sku LIKE 'ACME-DEMO-%' AND product.status = 'active'
           AND p.currency = 'USD' AND p.is_default AND p.valid_to IS NULL
           AND i.on_hand_quantity > 0 AND r.status = 'published') = 24
  )::int;")"
[[ "$(tr -d '[:space:]' <<<"$seed_check")" == 1 ]] || fail "deterministic commerce seed is incomplete"
schema_contract_check="$(psql_query "
  SELECT (
    (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'checkout_attempts'
        AND column_name = 'lease_token' AND is_nullable = 'NO') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_attempts'
        AND c.conname = 'checkout_attempts_lease_token_nonempty'
        AND c.contype = 'c') = 1
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'inventory_reservations'
        AND column_name IN ('expires_at', 'consumed_at', 'owner_request_id', 'owner_lease_token')) = 4
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'inventory_reservations'
        AND column_name = 'expires_at' AND is_nullable = 'NO') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'inventory_reservations'
        AND c.contype = 'c'
        AND c.conname IN ('inventory_reservations_status_check', 'inventory_reservations_lifecycle_check')) = 2
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'checkout_compensation_tasks'
        AND column_name IN ('checkout_request_id', 'checkout_lease_token',
                            'remote_operation_id', 'remote_operation_started_at',
                            'remote_operation_completed_at')) = 5
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_compensation_tasks'
        AND c.conname = 'checkout_compensation_tasks_remote_operation_check'
        AND c.contype = 'c') = 1
    AND to_regclass('public.idx_checkout_attempts_lease') IS NOT NULL
    AND to_regclass('public.idx_inventory_reservations_expiry') IS NOT NULL
    AND to_regclass('public.idx_inventory_reservations_owner') IS NOT NULL
    AND to_regclass('public.idx_checkout_compensation_fence') IS NOT NULL
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'outbox_events'
        AND column_name IN ('claim_token', 'claim_expires_at')) = 2
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'outbox_events'
        AND column_name IN ('traceparent', 'tracestate', 'baggage')
        AND is_nullable = 'NO') = 3
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'outbox_events'
        AND c.conname = 'outbox_claim_check' AND c.contype = 'c') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'outbox_events'
        AND c.conname = 'outbox_propagation_nonempty' AND c.contype = 'c') = 1
    AND to_regclass('public.idx_outbox_claim_expiry') IS NOT NULL
    AND to_regclass('public.checkout_payment_reconciliations') IS NOT NULL
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'checkout_payment_reconciliations'
        AND column_name IN ('request_id', 'authorize_request_id', 'status', 'lease_token',
                            'lease_expires_at', 'checkout_lease_token', 'completed_at')) = 7
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'checkout_payment_reconciliations'
        AND column_name = 'checkout_lease_token' AND is_nullable = 'NO') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_payment_reconciliations'
        AND c.conname = 'checkout_payment_reconciliations_lease_check'
        AND c.contype = 'c') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_payment_reconciliations'
        AND c.contype IN ('p', 'u')) = 2
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_payment_reconciliations'
        AND c.contype = 'p'
        AND pg_get_constraintdef(c.oid) = 'PRIMARY KEY (tenant_id, request_id)') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_payment_reconciliations'
        AND c.contype = 'f') = 3
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_payment_reconciliations'
        AND c.contype = 'c') = 9
    AND to_regclass('public.idx_checkout_payment_reconciliation_queue') IS NOT NULL
    AND to_regclass('public.idx_checkout_payment_reconciliation_payment') IS NOT NULL
    AND to_regclass('public.idx_checkout_payment_reconciliation_parent_generation') IS NOT NULL
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'checkout_payment_reconciliations'
        AND c.conname = 'checkout_payment_reconciliations_parent_generation_check'
        AND c.contype = 'c') = 1
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'payments'
        AND column_name IN ('pending_resolution', 'pending_reconciliation_attempts',
                            'pending_reconciliation_at', 'pending_resolution_at')) = 4
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'payments'
        AND column_name = 'pending_reconciliation_attempts' AND is_nullable = 'NO') = 1
    AND to_regclass('public.idx_payments_pending_reconciliation') IS NOT NULL
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'payments'
        AND c.contype = 'c'
        AND c.conname IN ('payments_pending_resolution_value_check',
                          'payments_pending_reconciliation_state_check',
                          'payments_pending_resolution_timestamp_check',
                          'payments_pending_reconciliation_attempts_check',
                          'payments_pending_reconciliation_lifecycle_check')) = 5
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'notification_deliveries'
        AND column_name IN ('next_attempt_at', 'lease_until', 'lease_token', 'last_error',
                            'dead_lettered_at', 'acknowledged_at', 'updated_at',
                            'traceparent', 'tracestate', 'baggage')) = 10
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'notification_deliveries'
        AND column_name IN ('next_attempt_at', 'updated_at') AND is_nullable = 'NO') = 2
    AND to_regclass('public.idx_notification_deliveries_dispatch_queue') IS NOT NULL
    AND to_regclass('public.idx_notification_deliveries_expired_leases') IS NOT NULL
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_deliveries'
        AND c.contype = 'c'
        AND c.conname IN ('notification_deliveries_status_check',
                          'notification_deliveries_delivered_at_check',
                          'notification_deliveries_dead_lettered_at_check',
                          'notification_deliveries_lease_check')) = 4
    AND to_regclass('public.notification_delivery_attempts') IS NOT NULL
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'notification_delivery_attempts'
        AND column_name IN ('id', 'delivery_id', 'tenant_id', 'attempt', 'outcome', 'error',
                            'started_at', 'completed_at')) = 8
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_delivery_attempts'
        AND c.contype = 'p'
        AND pg_get_constraintdef(c.oid) = 'PRIMARY KEY (id)') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_delivery_attempts'
        AND c.contype = 'u'
        AND pg_get_constraintdef(c.oid) LIKE '%(delivery_id, attempt)%') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_delivery_attempts'
        AND c.contype = 'f') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_delivery_attempts'
        AND c.contype = 'c') = 3
    AND to_regclass('public.idx_notification_delivery_attempts_tenant') IS NOT NULL
    AND to_regclass('public.notification_channel_messages') IS NOT NULL
    AND (SELECT count(*) FROM information_schema.columns
      WHERE table_schema = 'public' AND table_name = 'notification_channel_messages'
        AND column_name IN ('delivery_id', 'tenant_id', 'order_id', 'channel', 'payload',
                            'dispatched_at', 'acknowledged_at', 'acknowledgement_reference')) = 8
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_channel_messages'
        AND c.contype = 'p'
        AND pg_get_constraintdef(c.oid) = 'PRIMARY KEY (delivery_id)') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_channel_messages'
        AND c.contype = 'u'
        AND pg_get_constraintdef(c.oid) LIKE '%(tenant_id, delivery_id)%') = 1
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_channel_messages'
        AND c.contype = 'f') = 2
    AND (SELECT count(*) FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public' AND t.relname = 'notification_channel_messages'
        AND c.contype = 'c') = 2
  )::int;")"
[[ "$(tr -d '[:space:]' <<<"$schema_contract_check")" == 1 ]] || \
  fail "011-015 checkout fencing, inventory lifecycle, payment reconciliation, pending-payment, or notification schema is incomplete"

payment_provider_decision_schema_contract="$(psql_query "
  WITH expected_columns(table_name, column_name, data_type, required_nullability) AS (
    VALUES
      ('payments', 'provider_decision_verified_at', 'timestamp with time zone', 'YES'),
      ('payment_provider_authorization_decisions', 'tenant_id', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'payment_id', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'provider', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'provider_reference', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'amount_minor', 'integer', 'NO'),
      ('payment_provider_authorization_decisions', 'currency', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'outcome', 'text', 'NO'),
      ('payment_provider_authorization_decisions', 'verified_at', 'timestamp with time zone', 'NO'),
      ('payment_provider_authorization_decisions', 'created_at', 'timestamp with time zone', 'NO')
  ), actual_columns AS (
    SELECT table_name, column_name, data_type, is_nullable
    FROM information_schema.columns
    WHERE table_schema = 'public'
  ), expected_foreign_keys(label, child_columns, referenced_table, referenced_columns) AS (
    VALUES
      ('payment_provider_authorization_decisions (tenant_id) -> tenants (id) ON DELETE CASCADE',
       ARRAY['tenant_id']::text[], 'tenants', ARRAY['id']::text[]),
      ('payment_provider_authorization_decisions (tenant_id, payment_id) -> payments (tenant_id, id) ON DELETE CASCADE',
       ARRAY['tenant_id', 'payment_id']::text[], 'payments', ARRAY['tenant_id', 'id']::text[])
  ), actual_foreign_keys AS (
    SELECT
      ARRAY(
        SELECT child_column.attname::text
        FROM unnest(c.conkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute child_column
          ON child_column.attrelid = c.conrelid
         AND child_column.attnum = key.attnum
         AND NOT child_column.attisdropped
        ORDER BY key.ordinal
      ) AS child_columns,
      referenced_table.relname AS referenced_table,
      ARRAY(
        SELECT referenced_column.attname::text
        FROM unnest(c.confkey) WITH ORDINALITY AS key(attnum, ordinal)
        JOIN pg_attribute referenced_column
          ON referenced_column.attrelid = c.confrelid
         AND referenced_column.attnum = key.attnum
         AND NOT referenced_column.attisdropped
        ORDER BY key.ordinal
      ) AS referenced_columns,
      c.confdeltype AS delete_action
    FROM pg_constraint c
    JOIN pg_class child_table ON child_table.oid = c.conrelid
    JOIN pg_namespace child_schema ON child_schema.oid = child_table.relnamespace
    JOIN pg_class referenced_table ON referenced_table.oid = c.confrelid
    JOIN pg_namespace referenced_schema ON referenced_schema.oid = referenced_table.relnamespace
    WHERE c.contype = 'f'
      AND child_schema.nspname = 'public'
      AND child_table.relname = 'payment_provider_authorization_decisions'
      AND referenced_schema.nspname = 'public'
  ), actual_checks AS (
    SELECT pg_get_constraintdef(c.oid) AS definition
    FROM pg_constraint c
    JOIN pg_class t ON t.oid = c.conrelid
    JOIN pg_namespace n ON n.oid = t.relnamespace
    WHERE n.nspname = 'public'
      AND t.relname = 'payment_provider_authorization_decisions'
      AND c.contype = 'c'
  ), expected_checks(fragment) AS (
    VALUES
      ('amount_minor >= 0'),
      ('char_length(currency) = 3'),
      ('currency = upper(currency)'),
      ('outcome = ANY')
  ), actual_indexes AS (
    SELECT indexname, indexdef
    FROM pg_indexes
    WHERE schemaname = 'public'
      AND tablename = 'payment_provider_authorization_decisions'
  ), actual_unique_constraints AS (
    SELECT pg_get_constraintdef(c.oid) AS definition
    FROM pg_constraint c
    JOIN pg_class t ON t.oid = c.conrelid
    JOIN pg_namespace n ON n.oid = t.relnamespace
    WHERE n.nspname = 'public'
      AND t.relname = 'payment_provider_authorization_decisions'
      AND c.contype = 'u'
  ), actual_lookup_indexes AS (
    SELECT indexdef
    FROM pg_indexes
    WHERE schemaname = 'public'
      AND tablename = 'payment_provider_authorization_decisions'
      AND indexname = 'idx_payment_provider_decisions_lookup'
  )
  SELECT concat_ws(
    '; ',
    CASE WHEN (
      SELECT count(*)
      FROM pg_class table_row
      JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
      WHERE schema_row.nspname = 'public'
        AND table_row.relname = 'payment_provider_authorization_decisions'
        AND table_row.relkind = 'r'
    ) <> 1 THEN 'payment_provider_authorization_decisions expected exactly 1 base table, got '
      || (
        SELECT count(*)
        FROM pg_class table_row
        JOIN pg_namespace schema_row ON schema_row.oid = table_row.relnamespace
        WHERE schema_row.nspname = 'public'
          AND table_row.relname = 'payment_provider_authorization_decisions'
          AND table_row.relkind = 'r'
      )::text END,
    (
      SELECT string_agg(
        expected_columns.table_name || '.' || expected_columns.column_name
          || ' (expected ' || expected_columns.data_type
          || ', nullable=' || expected_columns.required_nullability || ')',
        ', ' ORDER BY expected_columns.table_name, expected_columns.column_name
      )
      FROM expected_columns
      LEFT JOIN actual_columns
        ON actual_columns.table_name = expected_columns.table_name
       AND actual_columns.column_name = expected_columns.column_name
      WHERE actual_columns.column_name IS NULL
         OR actual_columns.data_type <> expected_columns.data_type
         OR actual_columns.is_nullable <> expected_columns.required_nullability
    ),
    CASE WHEN (
      SELECT count(*)
      FROM information_schema.columns
      WHERE table_schema = 'public'
        AND table_name = 'payment_provider_authorization_decisions'
    ) <> 9 THEN 'payment_provider_authorization_decisions expected exactly 9 columns, got '
      || (
        SELECT count(*)
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'payment_provider_authorization_decisions'
      )::text END,
    CASE WHEN (
      SELECT count(*)
      FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public'
        AND t.relname = 'payment_provider_authorization_decisions'
        AND c.contype = 'p'
    ) <> 1 THEN 'payment_provider_authorization_decisions expected exactly 1 PRIMARY KEY, got '
      || (
        SELECT count(*)
        FROM pg_constraint c
        JOIN pg_class t ON t.oid = c.conrelid
        JOIN pg_namespace n ON n.oid = t.relnamespace
        WHERE n.nspname = 'public'
          AND t.relname = 'payment_provider_authorization_decisions'
          AND c.contype = 'p'
      )::text
    WHEN NOT EXISTS (
      SELECT 1
      FROM pg_constraint c
      JOIN pg_class t ON t.oid = c.conrelid
      JOIN pg_namespace n ON n.oid = t.relnamespace
      WHERE n.nspname = 'public'
        AND t.relname = 'payment_provider_authorization_decisions'
        AND c.contype = 'p'
        AND pg_get_constraintdef(c.oid) = 'PRIMARY KEY (tenant_id, payment_id)'
    ) THEN 'payment_provider_authorization_decisions PRIMARY KEY (tenant_id, payment_id) is missing'
    END,
    CASE WHEN (SELECT count(*) FROM actual_foreign_keys) <> 2
      THEN 'payment_provider_authorization_decisions expected exactly 2 foreign keys, got '
        || (SELECT count(*) FROM actual_foreign_keys)::text END,
    (
      SELECT string_agg(expected_foreign_keys.label, ', ' ORDER BY expected_foreign_keys.label)
      FROM expected_foreign_keys
      WHERE NOT EXISTS (
        SELECT 1
        FROM actual_foreign_keys
        WHERE actual_foreign_keys.child_columns = expected_foreign_keys.child_columns
          AND actual_foreign_keys.referenced_table = expected_foreign_keys.referenced_table
          AND actual_foreign_keys.referenced_columns = expected_foreign_keys.referenced_columns
          AND actual_foreign_keys.delete_action = 'c'
      )
    ),
    CASE WHEN (SELECT count(*) FROM actual_unique_constraints) <> 1
      THEN 'payment_provider_authorization_decisions expected exactly 1 UNIQUE constraint, got '
        || (SELECT count(*) FROM actual_unique_constraints)::text
    WHEN NOT EXISTS (
      SELECT 1
      FROM actual_unique_constraints
      WHERE definition = 'UNIQUE (tenant_id, provider, provider_reference)'
    ) THEN 'payment_provider_authorization_decisions UNIQUE (tenant_id, provider, provider_reference) is missing'
    END,
    CASE WHEN (SELECT count(*) FROM actual_checks) <> 3
      THEN 'payment_provider_authorization_decisions expected exactly 3 CHECK constraints, got '
        || (SELECT count(*) FROM actual_checks)::text END,
    (
      SELECT string_agg('payment_provider_authorization_decisions CHECK [' || expected_checks.fragment || ']', ', ' ORDER BY expected_checks.fragment)
      FROM expected_checks
      WHERE NOT EXISTS (
        SELECT 1
        FROM actual_checks
        WHERE actual_checks.definition LIKE '%' || expected_checks.fragment || '%'
      )
    ),
    CASE WHEN (SELECT count(*) FROM actual_indexes) <> 3
      THEN 'payment_provider_authorization_decisions expected exactly 3 indexes, got '
        || (SELECT count(*) FROM actual_indexes)::text END,
    CASE WHEN (SELECT count(*) FROM actual_lookup_indexes) <> 1
      THEN 'idx_payment_provider_decisions_lookup expected exactly 1 index, got '
        || (SELECT count(*) FROM actual_lookup_indexes)::text
    WHEN NOT EXISTS (
      SELECT 1
      FROM actual_lookup_indexes
      WHERE indexdef LIKE 'CREATE INDEX %'
        AND indexdef LIKE '%(tenant_id, provider, provider_reference, verified_at DESC)%'
        AND indexdef NOT LIKE '% WHERE %'
    ) THEN 'idx_payment_provider_decisions_lookup definition is missing or mismatched: '
      || COALESCE((SELECT string_agg(indexdef, ' | ') FROM actual_lookup_indexes), '<none>')
    END
  );
")"
[[ -z "$payment_provider_decision_schema_contract" ]] || \
  fail "012 payment provider authorization decision schema contract failed: $payment_provider_decision_schema_contract"
echo "PostgreSQL 011/012/013 fencing, inventory lifecycle, payment reconciliation, pending-payment provider-decision, and notification schema passed"
psql_query "SELECT 1 FROM public.orders LIMIT 1;" >/dev/null || fail "orders table is not queryable"
echo "PostgreSQL reachable: exact migration set and deterministic commerce seed passed"

curl -fsS --max-time "$HTTP_TIMEOUT" "$FLAGD_HEALTH_URL" >/dev/null || fail "flagd health failed"
echo "flagd health passed"

web_response="$TMP_DIR/web-home.html"
web_code="$(curl -sS --max-time "$HTTP_TIMEOUT" -o "$web_response" -w '%{http_code}' "$WEB_URL/" \
  2>"$TMP_DIR/web-home.curl.log" || true)"
[[ "$web_code" =~ ^2[0-9][0-9] ]] || fail "web home returned HTTP $web_code"
grep -Eq 'Parallax Commerce Lab|Commerce' "$web_response" || fail "web home did not render the commerce application"
echo "Web commerce surface passed"

graphql_post() {
  local label="$1" url="$2" payload="$3" response="$4" code
  local -a headers=(-H 'content-type: application/json')
  if [[ -n "${TRACEPARENT:-}" ]]; then
    headers+=(-H "traceparent: $TRACEPARENT" -H "tracestate: $TRACESTATE" -H "baggage: $BAGGAGE")
  fi
  code="$(curl -sS --max-time "$HTTP_TIMEOUT" -o "$response" -w '%{http_code}' \
    "${headers[@]}" --data-binary "$payload" "$url" \
    2>"$TMP_DIR/$label.curl.log" || true)"
  [[ "$code" =~ ^2[0-9][0-9] ]] || fail "$label returned HTTP $code"
  jq -e '(.errors // []) | length == 0' "$response" >/dev/null 2>&1 || \
    fail "$label returned GraphQL errors"
}

new_w3c_context() {
  local session_id="${1:-$SESSION_ID}" trace_id span_id
  trace_id="$(od -An -N16 -tx1 /dev/urandom | tr -d '[:space:]')"
  span_id="$(od -An -N8 -tx1 /dev/urandom | tr -d '[:space:]')"
  [[ "$trace_id" =~ ^[[:xdigit:]]{32}$ && "$span_id" =~ ^[[:xdigit:]]{16}$ ]] || \
    fail "could not generate a valid W3C trace context"
  [[ "$trace_id" != 00000000000000000000000000000000 && \
     "$span_id" != 0000000000000000 ]] || fail "generated an invalid zero W3C trace context"
  TRACEPARENT="00-${trace_id}-${span_id}-01"
  TRACESTATE="playground=commerce"
  BAGGAGE="tenant.id=${TENANT_ID},user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal,session.id=${session_id}"
}

base64url() {
  printf '%s' "$1" | base64 | tr '+/' '-_' | tr -d '=\r\n'
}

catalog_tenant_key="$(base64url "$TENANT_ID")"
catalog_sku_key="$(base64url "$SKU")"
catalog_cache_key="catalog:v2:product:${catalog_tenant_key}:${catalog_sku_key}"
new_w3c_context

if [[ "$MANAGE_STACK" == 1 ]]; then
  if ! redis_cli DEL "$catalog_cache_key" >"$TMP_DIR/catalog-cache-clear.log" 2>&1; then
    fail "could not clear the scoped catalog cache key"
  fi
  catalog_cache_exists_before="$(redis_cli EXISTS "$catalog_cache_key" 2>"$TMP_DIR/catalog-cache-before.err")" || \
    fail "could not verify the cold catalog cache state"
  [[ "$(tr -d '[:space:]' <<<"$catalog_cache_exists_before")" == 0 ]] || \
    fail "catalog cache key survived the cold-cache reset"
fi

catalog_query="$(printf 'query CatalogProduct { product(sku: "%s", tenantId: "%s", segment: "standard") { sku tenantId name priceMinor variants { sku } } }' "$SKU" "$TENANT_ID")"
catalog_payload="$(jq -cn --arg query "$catalog_query" \
  '{operationName:"CatalogProduct",query:$query}')"
catalog_cold_response="$TMP_DIR/catalog-cold.json"
catalog_warm_response="$TMP_DIR/catalog-warm.json"
new_w3c_context
CATALOG_COLD_TRACE_ID="${TRACEPARENT:3:32}"
graphql_post catalog-cold "$CATALOG_GRAPHQL_URL" "$catalog_payload" "$catalog_cold_response"
new_w3c_context
CATALOG_WARM_TRACE_ID="${TRACEPARENT:3:32}"
graphql_post catalog-warm "$CATALOG_GRAPHQL_URL" "$catalog_payload" "$catalog_warm_response"
if [[ "$MANAGE_STACK" == 1 ]] && ! cmp -s "$catalog_cold_response" "$catalog_warm_response"; then
  fail "catalog cold and warm responses differ"
fi
for catalog_response in "$catalog_cold_response" "$catalog_warm_response"; do
  jq -e --arg sku "$SKU" --arg tenant "$TENANT_ID" \
    '.data.product.sku == $sku
     and .data.product.tenantId == $tenant
     and (.data.product.priceMinor | type == "number")
     and (.data.product.variants | length >= 1)' \
    "$catalog_response" >/dev/null 2>&1 || fail "catalog GraphQL product read was incomplete"
done
CATALOG_PRICE_MINOR="$(jq -r '.data.product.priceMinor // empty' "$catalog_cold_response")"
[[ "$CATALOG_PRICE_MINOR" =~ ^[0-9]+$ ]] || fail "catalog returned an invalid seeded price"
catalog_cache_exists="$(redis_cli EXISTS "$catalog_cache_key" 2>"$TMP_DIR/catalog-cache.err")" || \
  fail "could not verify the warm catalog cache state"
[[ "$(tr -d '[:space:]' <<<"$catalog_cache_exists")" == 1 ]] || \
  fail "catalog warm read did not populate the scoped Redis cache"
if [[ "$MANAGE_STACK" == 1 ]]; then
  echo "Catalog GraphQL cold/warm reads passed"
else
  echo "Catalog GraphQL reads passed (external cache state retained)"
fi

# shellcheck disable=SC2016 # GraphQL variables must reach the downstream parser unchanged.
catalog_products_query='query CatalogProducts($tenantId: ID!, $search: String, $category: String, $sort: ProductSort!, $page: Int!, $size: Int!) {
  products(tenantId: $tenantId, search: $search, category: $category, sort: $sort, page: $page, size: $size, segment: "standard") {
    page size totalElements totalPages hasNext experience
    items {
      id tenantId slug sku name
      category { id tenantId slug name }
      variants { id tenantId productId sku price { id currency amountMinor } }
      reviews { id productId stars verifiedPurchase }
    }
  }
}'
catalog_page_zero_payload="$(jq -cn --arg query "$catalog_products_query" --arg tenant "$TENANT_ID" --arg sort NEWEST \
  --argjson page 0 --argjson size 1 \
  '{operationName:"CatalogProducts",query:$query,variables:{tenantId:$tenant,search:"assortment",category:null,sort:$sort,page:$page,size:$size}}')"
catalog_page_zero_response="$TMP_DIR/catalog-page-zero.json"
graphql_post catalog-page-zero "$CATALOG_GRAPHQL_URL" "$catalog_page_zero_payload" "$catalog_page_zero_response"
jq -e --arg tenant "$TENANT_ID" \
  '.data.products.items[0] as $product
   | .data.products.page == 0
   and .data.products.size == 1
   and .data.products.totalElements == 24
   and .data.products.totalPages == 24
   and .data.products.hasNext == true
   and (.data.products.items | length == 1)
   and .data.products.items[0].tenantId == $tenant
   and .data.products.items[0].sku == "ACME-DEMO-24"
   and .data.products.items[0].category.slug == "electronics"' \
  "$catalog_page_zero_response" >/dev/null 2>&1 || fail "catalog pagination page zero is incomplete"
catalog_page_zero_product_id="$(jq -r '.data.products.items[0].id // empty' "$catalog_page_zero_response")"

catalog_page_one_payload="$(jq -cn --arg query "$catalog_products_query" --arg tenant "$TENANT_ID" --arg sort NEWEST \
  --argjson page 1 --argjson size 1 \
  '{operationName:"CatalogProducts",query:$query,variables:{tenantId:$tenant,search:"assortment",category:null,sort:$sort,page:$page,size:$size}}')"
catalog_page_one_response="$TMP_DIR/catalog-page-one.json"
graphql_post catalog-page-one "$CATALOG_GRAPHQL_URL" "$catalog_page_one_payload" "$catalog_page_one_response"
jq -e --arg tenant "$TENANT_ID" \
  '.data.products.page == 1
   and .data.products.size == 1
   and .data.products.totalElements == 24
   and .data.products.totalPages == 24
   and .data.products.hasNext == true
   and (.data.products.items | length == 1)
   and .data.products.items[0].tenantId == $tenant
   and .data.products.items[0].sku == "ACME-DEMO-23"
   and .data.products.items[0].category.slug == "kitchen"' \
  "$catalog_page_one_response" >/dev/null 2>&1 || fail "catalog pagination page one is incomplete"
catalog_page_one_product_id="$(jq -r '.data.products.items[0].id // empty' "$catalog_page_one_response")"
[[ -n "$catalog_page_zero_product_id" && -n "$catalog_page_one_product_id" \
  && "$catalog_page_zero_product_id" != "$catalog_page_one_product_id" ]] || \
  fail "catalog pagination returned duplicate product identities"

catalog_batch_payload="$(jq -cn --arg query "$catalog_products_query" --arg tenant "$TENANT_ID" --arg sort NEWEST \
  --argjson page 0 --argjson size 2 \
  '{operationName:"CatalogProducts",query:$query,variables:{tenantId:$tenant,search:"assortment",category:null,sort:$sort,page:$page,size:$size}}')"
catalog_batch_response="$TMP_DIR/catalog-batch.json"
graphql_post catalog-batch "$CATALOG_GRAPHQL_URL" "$catalog_batch_payload" "$catalog_batch_response"
jq -e --arg tenant "$TENANT_ID" \
  '.data.products.page == 0
   and .data.products.size == 2
   and .data.products.totalElements == 24
   and .data.products.totalPages == 12
   and .data.products.hasNext == true
   and (.data.products.items | length == 2)
   and all(.data.products.items[]; . as $product |
     $product.tenantId == $tenant
     and $product.category.tenantId == $tenant
     and ($product.variants | length >= 1)
     and all($product.variants[]; .tenantId == $tenant and .productId == $product.id)
     and ($product.reviews | length >= 1)
     and all($product.reviews[]; .productId == $product.id))' \
  "$catalog_batch_response" >/dev/null 2>&1 || fail "catalog nested variant/review batch response was incomplete"

catalog_filter_payload="$(jq -cn --arg query "$catalog_products_query" --arg tenant "$TENANT_ID" \
  --arg search "Product 24" --arg category electronics --arg sort PRICE_DESC \
  --argjson page 0 --argjson size 1 \
  '{operationName:"CatalogProducts",query:$query,variables:{tenantId:$tenant,search:$search,category:$category,sort:$sort,page:$page,size:$size}}')"
catalog_filter_response="$TMP_DIR/catalog-filter.json"
graphql_post catalog-filter "$CATALOG_GRAPHQL_URL" "$catalog_filter_payload" "$catalog_filter_response"
jq -e --arg tenant "$TENANT_ID" \
  '.data.products.items[0] as $product
   | .data.products.page == 0
   and .data.products.size == 1
   and .data.products.totalElements == 1
   and .data.products.totalPages == 1
   and .data.products.hasNext == false
   and (.data.products.items | length == 1)
   and $product.tenantId == $tenant
   and $product.sku == "ACME-DEMO-24"
   and $product.category.slug == "electronics"
   and $product.category.tenantId == $tenant
   and ($product.variants | length == 1)
   and all($product.variants[]; .tenantId == $tenant and .productId == $product.id)
   and ($product.reviews | length == 1)
   and $product.reviews[0].stars == 5' \
  "$catalog_filter_response" >/dev/null 2>&1 || fail "catalog search/filter/nested response was incomplete"

catalog_category_payload="$(jq -cn --arg query "$catalog_products_query" --arg tenant "$TENANT_ID" \
  --arg search assortment --arg category electronics --arg sort PRICE_ASC \
  --argjson page 0 --argjson size 2 \
  '{operationName:"CatalogProducts",query:$query,variables:{tenantId:$tenant,search:$search,category:$category,sort:$sort,page:$page,size:$size}}')"
catalog_category_response="$TMP_DIR/catalog-category.json"
graphql_post catalog-category "$CATALOG_GRAPHQL_URL" "$catalog_category_payload" "$catalog_category_response"
jq -e --arg tenant "$TENANT_ID" \
  'all(.data.products.items[];
     . as $product
     | $product.tenantId == $tenant
     and $product.category.slug == "electronics"
     and $product.category.tenantId == $tenant
     and ($product.variants | length >= 1)
     and all($product.variants[]; .tenantId == $tenant and .productId == $product.id)
     and ($product.reviews | length >= 1)
     and all($product.reviews[]; .productId == $product.id))
   and .data.products.page == 0
   and .data.products.size == 2
   and .data.products.totalElements == 12
   and .data.products.totalPages == 6
   and .data.products.hasNext == true
   and (.data.products.items | length == 2)
   and .data.products.items[0].sku == "ACME-DEMO-02"
   and .data.products.items[1].sku == "ACME-DEMO-04"' \
  "$catalog_category_response" >/dev/null 2>&1 || fail "catalog category filter/sort response was incomplete"
echo "Catalog category, pagination, sort, filter, nested fields, and batch cardinality passed"

cart_add_payload="$(jq -cn \
  --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" --arg sku "$SKU" --arg session "$SESSION_ID" \
  '{
    operationName: "AddCartItem",
    query: "mutation AddCartItem($input: AddCartItemInput!) { addCartItem(input: $input) { cartId sku quantityAdded unitPriceMinor } }",
    variables: {input: {tenantId: $tenant, customerId: $customer, sessionId: $session, sku: $sku, quantity: 1, currencyCode: "USD"}}
  }')"
cart_add_response="$TMP_DIR/storefront-cart-add.json"
graphql_post storefront-cart-add "$STOREFRONT_GRAPHQL_URL" "$cart_add_payload" "$cart_add_response"
CART_ID="$(jq -r '.data.addCartItem.cartId // empty' "$cart_add_response")"
[[ "$CART_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "Storefront cart mutation returned an unsafe cart id"
jq -e --arg sku "$SKU" \
  '.data.addCartItem.sku == $sku and .data.addCartItem.quantityAdded == 1 and (.data.addCartItem.unitPriceMinor | type == "number")' \
  "$cart_add_response" >/dev/null 2>&1 || fail "Storefront cart mutation was incomplete"
cart_query_payload="$(jq -cn --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" \
  '{
    operationName: "StorefrontCart",
    query: "query StorefrontCart($tenantId: String, $customerId: String) { cart(tenantId: $tenantId, customerId: $customerId) { id status currency items { sku quantity unitPriceMinor lineTotalMinor } } }",
    variables: {tenantId: $tenant, customerId: $customer}
  }')"
cart_query_response="$TMP_DIR/storefront-cart-query.json"
graphql_post storefront-cart-query "$STOREFRONT_GRAPHQL_URL" "$cart_query_payload" "$cart_query_response"
jq -e --arg cart "$CART_ID" --arg sku "$SKU" \
  '.data.cart.id == $cart
   and .data.cart.status == "active"
   and .data.cart.currency == "USD"
   and (.data.cart.items | length == 1)
   and .data.cart.items[0].sku == $sku
   and .data.cart.items[0].quantity == 1
   and (.data.cart.items[0].unitPriceMinor | type == "number")' \
  "$cart_query_response" >/dev/null 2>&1 || fail "Storefront cart query was incomplete or unscoped"
echo "Storefront cart mutation/query passed"

pricing_cache_pattern="pricing:quote:${TENANT_ID}:${CUSTOMER_ID}:*"
if [[ "$MANAGE_STACK" == 1 ]]; then
  pricing_cache_keys_before="$(redis_cli --scan --pattern "$pricing_cache_pattern" 2>"$TMP_DIR/pricing-cache-clear.err")" || \
    fail "could not inspect the scoped pricing cache before the cold read"
  if [[ -n "$pricing_cache_keys_before" ]]; then
    while IFS= read -r pricing_cache_key; do
      [[ -n "$pricing_cache_key" ]] || continue
      redis_cli DEL "$pricing_cache_key" >/dev/null || fail "could not clear scoped pricing cache key"
    done <<<"$pricing_cache_keys_before"
  fi
  pricing_cache_keys_after_clear="$(redis_cli --scan --pattern "$pricing_cache_pattern" 2>"$TMP_DIR/pricing-cache-after-clear.err")" || \
    fail "could not verify the cold pricing cache state"
  [[ -z "$(awk 'NF {print; exit}' <<<"$pricing_cache_keys_after_clear")" ]] || \
    fail "pricing cache retained a scoped key after the cold-cache reset"
fi

storefront_payload="$(jq -cn \
  --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" --arg sku "$SKU" \
  '{
    operationName: "StorefrontQuote",
    query: "query StorefrontQuote($input: QuoteInput!) { quote(input: $input) { quoteId status lines { sku quantity unitPrice { currencyCode amountMinor } lineTotal { currencyCode amountMinor } } subtotal { currencyCode amountMinor } discountTotal { currencyCode amountMinor } taxTotal { currencyCode amountMinor } grandTotal { currencyCode amountMinor } validForSeconds pricingVersion } }",
    variables: {input: {tenantId: $tenant, customerId: $customer, currencyCode: "USD", items: [{sku: $sku, quantity: 2}]}}
  }')"
storefront_quote_one="$TMP_DIR/storefront-quote-one.json"
storefront_quote_two="$TMP_DIR/storefront-quote-two.json"
new_w3c_context
PRICING_COLD_TRACE_ID="${TRACEPARENT:3:32}"
graphql_post storefront-quote-one "$STOREFRONT_GRAPHQL_URL" "$storefront_payload" "$storefront_quote_one"
new_w3c_context
PRICING_WARM_TRACE_ID="${TRACEPARENT:3:32}"
graphql_post storefront-quote-two "$STOREFRONT_GRAPHQL_URL" "$storefront_payload" "$storefront_quote_two"
if [[ "$MANAGE_STACK" == 1 ]] && ! cmp -s "$storefront_quote_one" "$storefront_quote_two"; then
  fail "pricing cold and warm responses differ"
fi
for quote_response in "$storefront_quote_one" "$storefront_quote_two"; do
  jq -e --arg sku "$SKU" \
    '.data.quote.status == "QUOTE_STATUS_READY"
     and (.data.quote.quoteId | type == "string" and length > 0)
     and (.data.quote.pricingVersion | type == "string" and length > 0)
     and (.data.quote.lines | length >= 1)
     and .data.quote.lines[0].sku == $sku' \
    "$quote_response" >/dev/null 2>&1 || fail "Storefront pricing GraphQL quote was incomplete"
done
pricing_cache_keys="$(redis_cli --scan --pattern "$pricing_cache_pattern" \
  2>"$TMP_DIR/pricing-cache.err")" || fail "could not inspect the populated pricing cache"
[[ -n "$pricing_cache_keys" ]] || fail "Storefront pricing read did not populate the scoped Redis cache"
if [[ "$MANAGE_STACK" == 1 ]]; then
  pricing_cache_key_count="$(awk 'NF {count++} END {print count + 0}' <<<"$pricing_cache_keys")"
  [[ "$pricing_cache_key_count" == 1 ]] || \
    fail "Storefront pricing cache did not produce exactly one scoped key (got $pricing_cache_key_count)"
  pricing_cache_key="$(sed -n '1p' <<<"$pricing_cache_keys")"
  pricing_cache_exists="$(redis_cli EXISTS "$pricing_cache_key" 2>"$TMP_DIR/pricing-cache-exists.err" || true)"
  [[ "$(tr -d '[:space:]' <<<"$pricing_cache_exists")" == 1 ]] || fail "pricing cache key disappeared"
  echo "Storefront pricing GraphQL cold/warm reads passed"
else
  echo "Storefront pricing GraphQL passed (external cache state retained)"
fi
assert_cache_telemetry

if [[ "$MANAGE_STACK" == 1 ]]; then
  run_bounded redis-outage-stop "${COMPOSE[@]}" stop redis || fail "could not stop Redis for fallback proof"
  new_w3c_context
  redis_catalog_outage_response="$TMP_DIR/catalog-redis-outage.json"
  graphql_post catalog-redis-outage "$CATALOG_GRAPHQL_URL" "$catalog_payload" "$redis_catalog_outage_response"
  jq -e --arg sku "$SKU" --arg tenant "$TENANT_ID" \
    '.data.product.sku == $sku and .data.product.tenantId == $tenant
     and (.data.product.priceMinor | type == "number")' \
    "$redis_catalog_outage_response" >/dev/null 2>&1 || \
    fail "catalog did not fall back to PostgreSQL while Redis was unavailable"

  redis_pricing_outage_response="$TMP_DIR/pricing-redis-outage.json"
  graphql_post pricing-redis-outage "$STOREFRONT_GRAPHQL_URL" "$storefront_payload" "$redis_pricing_outage_response"
  jq -e --arg sku "$SKU" \
    '.data.quote.status == "QUOTE_STATUS_READY" and .data.quote.lines[0].sku == $sku' \
    "$redis_pricing_outage_response" >/dev/null 2>&1 || \
    fail "pricing did not fall back to PostgreSQL while Redis was unavailable"
  run_bounded redis-outage-start "${COMPOSE[@]}" start redis || fail "could not restart Redis after fallback proof"
  wait_for_healthy redis || fail "Redis did not recover after fallback proof"
  echo "Redis outage fallback passed for catalog and pricing"
else
  echo "Redis outage fallback skipped in external mode"
fi

send_checkout() {
  local request_id="$1" response="$2" use_cart="${3:-0}" payment_token="${4:-tok_visa}" quantity="${5:-1}"
  local checkout_session_id="${6:-$SESSION_ID}"
  new_w3c_context "$checkout_session_id"
  jq -cn \
    --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" --arg sku "$SKU" \
    --arg request_id "$request_id" --arg session_id "$checkout_session_id" \
    --arg cart_id "${CART_ID:-}" --arg payment_token "$payment_token" --arg use_cart "$use_cart" \
    --arg quantity "$quantity" \
    '{tenant_id:$tenant, customer_id:$customer, items:[{sku:$sku, quantity:($quantity | tonumber)}],
      session_id:$session_id, cart_id:(if $use_cart == "1" then $cart_id else null end),
      currency_code:"USD", payment_method_token:$payment_token, payment_method_type:"card",
      request_id:$request_id, segment:"standard", tier:"standard", region:"us-east-1",
      priority:"normal"}' >"$TMP_DIR/checkout-request.json"
  [[ "$(jq -r '.session_id // empty' "$TMP_DIR/checkout-request.json")" == "$checkout_session_id" \
    && "$BAGGAGE" == *"session.id=${checkout_session_id}"* ]] || \
    fail "checkout feature probe created inconsistent session body and W3C baggage"
  CHECKOUT_HTTP_CODE="$(curl -sS --max-time "$HTTP_TIMEOUT" -o "$response" -w '%{http_code}' \
    -X POST "$CHECKOUT_URL/checkout" \
    -H 'content-type: application/json' \
    -H "traceparent: $TRACEPARENT" \
    -H "tracestate: $TRACESTATE" \
    -H "baggage: $BAGGAGE" \
    --data-binary @"$TMP_DIR/checkout-request.json" \
    2>"$TMP_DIR/checkout.curl.log" || true)"
}

send_storefront_checkout() {
  local request_id="$1" response="$2"
  new_w3c_context
  storefront_checkout_payload="$(jq -cn \
    --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" --arg sku "$SKU" \
    --arg request_id "$request_id" --arg session_id "$SESSION_ID" --arg cart_id "$CART_ID" \
    '{
      operationName: "StorefrontCheckout",
      query: "mutation StorefrontCheckout($input: CheckoutInput!) { checkout(input: $input) { orderId status paymentStatus currency totalMinor eventKey featureVariant } }",
      variables: {input: {tenantId: $tenant, customerId: $customer, cartId: $cart_id, sessionId: $session_id, currencyCode: "USD", paymentMethodToken: "tok_visa", paymentMethodType: "card", segment: "standard", requestId: $request_id, items: [{sku: $sku, quantity: 1}]}}
    }')"
  graphql_post storefront-checkout "$STOREFRONT_GRAPHQL_URL" "$storefront_checkout_payload" "$response"
}

assert_paid_checkout() {
  local response="$1" expected_variant="$2"
  jq -e --arg expected "$expected_variant" \
    '(.status == "paid"
      and .payment_status == "captured"
      and (.order_id | type == "string" and length > 0)
      and .feature_variant == $expected)
    or (.data.checkout.status == "paid"
      and .data.checkout.paymentStatus == "captured"
      and (.data.checkout.orderId | type == "string" and length > 0)
      and .data.checkout.featureVariant == $expected)' \
    "$response" >/dev/null 2>&1 || fail "checkout did not return a paid response with the expected feature variant"
}

PRIMARY_REQUEST_ID="commerce-verify-${RUN_ID}"
[[ "$PRIMARY_REQUEST_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "generated checkout request id is unsafe"
PRIMARY_CHECKOUT_RESPONSE="$TMP_DIR/checkout-primary.json"
send_storefront_checkout "$PRIMARY_REQUEST_ID" "$PRIMARY_CHECKOUT_RESPONSE"
assert_paid_checkout "$PRIMARY_CHECKOUT_RESPONSE" "$EXPECTED_CHECKOUT_VARIANT"
PRIMARY_TRACEPARENT="$TRACEPARENT"
PRIMARY_TRACESTATE="$TRACESTATE"
PRIMARY_BAGGAGE="$BAGGAGE"
[[ "$PRIMARY_BAGGAGE" == *"tenant.id=${TENANT_ID}"* \
  && "$PRIMARY_BAGGAGE" == *"customer.segment=standard"* \
  && "$PRIMARY_BAGGAGE" == *"region=us-east-1"* \
  && "$PRIMARY_BAGGAGE" == *"request.priority=normal"* \
  && "$PRIMARY_BAGGAGE" == *"session.id=${PRIMARY_SESSION_ID}"* ]] || \
  fail "primary checkout did not carry the expected safe baggage"
CHECKOUT_FEATURE_VARIANT="$(jq -r '.feature_variant // .data.checkout.featureVariant // empty' "$PRIMARY_CHECKOUT_RESPONSE")"
ORDER_ID="$(jq -r '.order_id // .data.checkout.orderId // empty' "$PRIMARY_CHECKOUT_RESPONSE")"
EXPECTED_CURRENCY="$(jq -r '(.currency // .data.checkout.currency // empty)' "$PRIMARY_CHECKOUT_RESPONSE")"
EXPECTED_TOTAL_MINOR="$(jq -r '(.total_minor // .data.checkout.totalMinor // empty) | tostring' "$PRIMARY_CHECKOUT_RESPONSE")"
[[ "$ORDER_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "checkout returned an unsafe order id"
FULFILLMENT_CLAIM_KEY="fulfillment:${TENANT_ID}:${ORDER_ID}"
[[ "$CHECKOUT_FEATURE_VARIANT" == control || "$CHECKOUT_FEATURE_VARIANT" == orchestrated ]] || \
  fail "checkout returned an unknown feature variant"
[[ "$EXPECTED_CURRENCY" == USD && "$EXPECTED_TOTAL_MINOR" =~ ^[0-9]+$ ]] || \
  fail "checkout returned invalid money fields"
ORDER_EVENT_KEY="${ORDER_ID}:paid"
[[ "$ORDER_EVENT_KEY" =~ ^[A-Za-z0-9._:-]+$ && ${#ORDER_EVENT_KEY} -le 256 ]] || \
  fail "generated event key is unsafe"
echo "Storefront GraphQL checkout passed with W3C traceparent/tracestate/baggage"

REPLAY_CHECKOUT_RESPONSE="$TMP_DIR/checkout-replay.json"
send_storefront_checkout "$PRIMARY_REQUEST_ID" "$REPLAY_CHECKOUT_RESPONSE"
assert_paid_checkout "$REPLAY_CHECKOUT_RESPONSE" "$EXPECTED_CHECKOUT_VARIANT"
REPLAY_ORDER_ID="$(jq -r '.order_id // .data.checkout.orderId // empty' "$REPLAY_CHECKOUT_RESPONSE")"
REPLAY_EVENT_KEY="$(jq -r '.event_key // .data.checkout.eventKey // empty' "$REPLAY_CHECKOUT_RESPONSE")"
[[ "$REPLAY_ORDER_ID" == "$ORDER_ID" && "$REPLAY_EVENT_KEY" == "$ORDER_EVENT_KEY" ]] || \
  fail "replaying the same checkout request changed its durable identity"
cmp -s "$PRIMARY_CHECKOUT_RESPONSE" "$REPLAY_CHECKOUT_RESPONSE" || \
  fail "replaying the same checkout request changed its response"
echo "Checkout idempotency replay returned the original order/event identity"

CONFLICT_CHECKOUT_RESPONSE="$TMP_DIR/checkout-conflict.json"
send_checkout "$PRIMARY_REQUEST_ID" "$CONFLICT_CHECKOUT_RESPONSE" 1 tok_visa 2
[[ "$CHECKOUT_HTTP_CODE" == 409 ]] || fail "changed checkout replay returned HTTP $CHECKOUT_HTTP_CODE instead of 409"
jq -e '.error == "checkout_request_conflict"' "$CONFLICT_CHECKOUT_RESPONSE" >/dev/null 2>&1 || \
  fail "changed checkout replay did not return a typed request conflict"
echo "Checkout idempotency rejected a changed fingerprint without mutation"

DECLINE_REQUEST_ID="commerce-verify-decline-${RUN_ID}"
DECLINE_SESSION_ID="session-${RUN_ID}-decline"
[[ "$DECLINE_SESSION_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "generated decline session id is unsafe"
DECLINE_CHECKOUT_RESPONSE="$TMP_DIR/checkout-decline.json"
SESSION_ID="$DECLINE_SESSION_ID"
send_checkout "$DECLINE_REQUEST_ID" "$DECLINE_CHECKOUT_RESPONSE" 0 tok_decline
SESSION_ID="$PRIMARY_SESSION_ID"
[[ "$CHECKOUT_HTTP_CODE" == 402 ]] || fail "payment decline returned HTTP $CHECKOUT_HTTP_CODE instead of 402"
jq -e '.error == "payment_declined" and (.message | type == "string" and length > 0)' \
  "$DECLINE_CHECKOUT_RESPONSE" >/dev/null 2>&1 || fail "payment decline was not a typed checkout error"
echo "Payment decline produced a typed 402 response"

FULFILLMENT_RESPONSE="$TMP_DIR/fulfillment-verify.json"
fulfillment_deadline=$((SECONDS + ASYNC_TIMEOUT))
fulfillment_ready=0
while (( SECONDS < fulfillment_deadline )); do
  fulfillment_code="$(curl -sS --max-time "$HTTP_TIMEOUT" \
    -o "$FULFILLMENT_RESPONSE" -w '%{http_code}' \
    "${FULFILLMENT_AUTH_HEADERS[@]}" \
    "${FULFILLMENT_URL%/}/verify/order?order=$ORDER_ID" \
    2>"$TMP_DIR/fulfillment.curl.log" || true)"
  if [[ "$fulfillment_code" == 200 ]] && jq -e \
    '.ready == true and .fulfillment_status == "completed"
     and (.shipment_id | type == "string" and length > 0)
     and (.notification_deliveries >= 1)
     and .notification_status == "delivered"
     and .notification_channel == "in_app"
     and .notification_provider == "in_app_durable_sink"
     and (.notification_acknowledgement | type == "string" and length > 0)' \
    "$FULFILLMENT_RESPONSE" >/dev/null 2>&1; then
    fulfillment_ready=1
    break
  fi
  sleep 1
done
(( fulfillment_ready == 1 )) || fail "async fulfillment/notification processing did not complete within ${ASYNC_TIMEOUT}s"
SHIPMENT_ID="$(jq -r '.shipment_id // empty' "$FULFILLMENT_RESPONSE")"
[[ "$SHIPMENT_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "fulfillment returned an unsafe shipment id"
echo "Async fulfillment and notification polling passed"

new_w3c_context "$PRIMARY_SESSION_ID"
storefront_order_payload="$(jq -cn --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" --arg order "$ORDER_ID" \
  '{
    operationName: "StorefrontOrder",
    query: "query StorefrontOrder($orderId: String!, $tenantId: String, $customerId: String) { order(orderId: $orderId, tenantId: $tenantId, customerId: $customerId) { id tenantId customerId status paymentStatus currency subtotalMinor discountMinor taxMinor shippingMinor totalMinor items { sku quantity unitPriceMinor discountMinor lineTotalMinor } } }",
    variables: {orderId: $order, tenantId: $tenant, customerId: $customer}
  }')"
storefront_order_response="$TMP_DIR/storefront-order.json"
graphql_post storefront-order "$STOREFRONT_GRAPHQL_URL" "$storefront_order_payload" "$storefront_order_response"
jq -e --arg order "$ORDER_ID" --arg tenant "$TENANT_ID" --arg customer "$CUSTOMER_ID" --arg sku "$SKU" \
  --arg total "$EXPECTED_TOTAL_MINOR" --arg subtotal "$CATALOG_PRICE_MINOR" \
  '.data.order.id == $order
   and .data.order.tenantId == $tenant
   and .data.order.customerId == $customer
   and (.data.order.status == "paid" or .data.order.status == "processing"
        or .data.order.status == "shipped" or .data.order.status == "delivered")
   and .data.order.paymentStatus == "captured"
   and .data.order.currency == "USD"
   and .data.order.subtotalMinor == $subtotal
   and .data.order.discountMinor == "0"
   and .data.order.taxMinor == "0"
   and .data.order.shippingMinor == "0"
   and .data.order.totalMinor == $total
   and (.data.order.items | length == 1)
   and .data.order.items[0].sku == $sku
   and .data.order.items[0].quantity == 1
   and .data.order.items[0].unitPriceMinor == $subtotal
   and .data.order.items[0].discountMinor == "0"
   and .data.order.items[0].lineTotalMinor == $total' \
  "$storefront_order_response" >/dev/null 2>&1 || fail "Storefront order query was incomplete"
echo "Storefront order query passed"

psql_checkout_query() {
  {
    printf "SET lock_timeout = '%sms';\n" "$PG_LOCK_TIMEOUT_MS"
    printf "SET statement_timeout = '%sms';\n" "$PG_STATEMENT_TIMEOUT_MS"
    printf '%s\n' "$1"
  } | "${PSQL[@]}" \
    -v "tenant_id=$TENANT_ID" \
    -v "customer_id=$CUSTOMER_ID" \
    -v "order_id=$ORDER_ID" \
    -v "event_key=$ORDER_EVENT_KEY" \
    -v "fulfillment_claim_key=$FULFILLMENT_CLAIM_KEY" \
    -v "cart_id=$CART_ID" \
    -v "shipment_id=$SHIPMENT_ID" \
    -v "request_id=$PRIMARY_REQUEST_ID" \
    -v "decline_request_id=$DECLINE_REQUEST_ID" \
    -v "feature_variant=$CHECKOUT_FEATURE_VARIANT" \
    -v "sku=$SKU" \
    -v "session_id=$PRIMARY_SESSION_ID" \
    -v "trace_id=${PRIMARY_TRACEPARENT:3:32}" \
    -v "tracestate=$PRIMARY_TRACESTATE" \
    -v "expected_currency=$EXPECTED_CURRENCY" \
    -v "expected_total_minor=$EXPECTED_TOTAL_MINOR" \
    -v "expected_subtotal_minor=$CATALOG_PRICE_MINOR" \
    -Atq
}

payment_grpc_call() {
  local method="$1" request="$2" response="$3" error_log="$4"
  local headers_file="$TMP_DIR/payment-grpc.headers"
  if [[ ! -f "$headers_file" ]]; then
    printf 'traceparent: %s\ntracestate: %s\nbaggage: %s\n' \
      "$PRIMARY_TRACEPARENT" "$PRIMARY_TRACESTATE" "$PRIMARY_BAGGAGE" >"$headers_file"
  fi
  buf --timeout "${HTTP_TIMEOUT}s" curl \
    --schema "$ROOT/proto/payment.proto" --reflect=false \
    --protocol grpc --http2-prior-knowledge --connect-timeout "$HTTP_TIMEOUT" \
    --emit-defaults \
    --header "@$headers_file" \
    --data "@$request" \
    -o "$response" \
    "${PAYMENT_GRPC_URL%/}/playground.payment.v1.Payment/$method" \
    2>"$error_log"
}

PRIMARY_PAYMENT_ID="$(psql_checkout_query "
  SELECT payment_id
    FROM payment_operation_requests
   WHERE tenant_id = :'tenant_id'
     AND request_id = (:'request_id' || ':authorize')
     AND operation = 'authorize'
     AND payment_id IS NOT NULL;
")"
PRIMARY_PAYMENT_ID="$(tr -d '[:space:]' <<<"$PRIMARY_PAYMENT_ID")"
[[ "$PRIMARY_PAYMENT_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || fail "primary checkout did not persist a safe payment id"

PAYMENT_AUTHORIZE_REQUEST="$TMP_DIR/payment-authorize.json"
jq -cn --arg request_id "${PRIMARY_REQUEST_ID}:authorize" --arg order_id "$ORDER_ID" \
  --arg tenant "$TENANT_ID" --arg amount "$EXPECTED_TOTAL_MINOR" \
  '{requestId:$request_id, merchantReference:$order_id,
    amount:{currencyCode:"USD", amountMinor:$amount},
    paymentMethod:{type:"PAYMENT_METHOD_TYPE_CARD", token:"tok_visa"}, tenantId:$tenant}' \
  >"$PAYMENT_AUTHORIZE_REQUEST"
PAYMENT_AUTHORIZE_REPLAY_RESPONSE="$TMP_DIR/payment-authorize-replay.json"
payment_grpc_call Authorize "$PAYMENT_AUTHORIZE_REQUEST" "$PAYMENT_AUTHORIZE_REPLAY_RESPONSE" \
  "$TMP_DIR/payment-authorize-replay.err" || fail "Payment gRPC authorize replay failed"
jq -e --arg payment "$PRIMARY_PAYMENT_ID" \
  '.operationStatus == "PAYMENT_OPERATION_STATUS_SUCCEEDED"
   and .payment.paymentId == $payment
   and .payment.status == "PAYMENT_STATUS_AUTHORIZED"' \
  "$PAYMENT_AUTHORIZE_REPLAY_RESPONSE" >/dev/null 2>&1 || \
  fail "Payment gRPC authorize replay returned the wrong durable response"
PAYMENT_AUTHORIZE_REPLAY_AGAIN="$TMP_DIR/payment-authorize-replay-again.json"
payment_grpc_call Authorize "$PAYMENT_AUTHORIZE_REQUEST" "$PAYMENT_AUTHORIZE_REPLAY_AGAIN" \
  "$TMP_DIR/payment-authorize-replay-again.err" || fail "Payment gRPC authorize retry failed"
cmp -s "$PAYMENT_AUTHORIZE_REPLAY_RESPONSE" "$PAYMENT_AUTHORIZE_REPLAY_AGAIN" || \
  fail "Payment gRPC authorize retry changed the stored response"

PAYMENT_CAPTURE_REQUEST="$TMP_DIR/payment-capture.json"
jq -cn --arg request_id "${PRIMARY_REQUEST_ID}:capture" --arg payment "$PRIMARY_PAYMENT_ID" \
  --arg tenant "$TENANT_ID" --arg amount "$EXPECTED_TOTAL_MINOR" \
  '{requestId:$request_id, paymentId:$payment,
    amount:{currencyCode:"USD", amountMinor:$amount}, tenantId:$tenant}' \
  >"$PAYMENT_CAPTURE_REQUEST"
PAYMENT_CAPTURE_REPLAY_RESPONSE="$TMP_DIR/payment-capture-replay.json"
payment_grpc_call Capture "$PAYMENT_CAPTURE_REQUEST" "$PAYMENT_CAPTURE_REPLAY_RESPONSE" \
  "$TMP_DIR/payment-capture-replay.err" || fail "Payment gRPC capture replay failed"
jq -e --arg payment "$PRIMARY_PAYMENT_ID" --arg amount "$EXPECTED_TOTAL_MINOR" \
  '.operationStatus == "PAYMENT_OPERATION_STATUS_SUCCEEDED"
   and .payment.paymentId == $payment
   and .payment.status == "PAYMENT_STATUS_CAPTURED"
   and .payment.capturedAmount.amountMinor == $amount' \
  "$PAYMENT_CAPTURE_REPLAY_RESPONSE" >/dev/null 2>&1 || \
  fail "Payment gRPC capture replay returned the wrong durable response"
PAYMENT_CAPTURE_REPLAY_AGAIN="$TMP_DIR/payment-capture-replay-again.json"
payment_grpc_call Capture "$PAYMENT_CAPTURE_REQUEST" "$PAYMENT_CAPTURE_REPLAY_AGAIN" \
  "$TMP_DIR/payment-capture-replay-again.err" || fail "Payment gRPC capture retry failed"
cmp -s "$PAYMENT_CAPTURE_REPLAY_RESPONSE" "$PAYMENT_CAPTURE_REPLAY_AGAIN" || \
  fail "Payment gRPC capture retry changed the stored response"

PAYMENT_AUTHORIZE_CONFLICT_REQUEST="$TMP_DIR/payment-authorize-conflict.json"
jq -cn --arg request_id "${PRIMARY_REQUEST_ID}:authorize" --arg order_id "$ORDER_ID" \
  --arg tenant "$TENANT_ID" --arg amount "$EXPECTED_TOTAL_MINOR" \
  '{requestId:$request_id, merchantReference:$order_id,
    amount:{currencyCode:"USD", amountMinor:$amount},
    paymentMethod:{type:"PAYMENT_METHOD_TYPE_CARD", token:"tok_conflict"}, tenantId:$tenant}' \
  >"$PAYMENT_AUTHORIZE_CONFLICT_REQUEST"
if payment_grpc_call Authorize "$PAYMENT_AUTHORIZE_CONFLICT_REQUEST" \
  "$TMP_DIR/payment-authorize-conflict.out" "$TMP_DIR/payment-authorize-conflict.err"; then
  fail "Payment gRPC changed-fingerprint replay unexpectedly succeeded"
fi
grep -Eiq 'ALREADY_EXISTS|AlreadyExists|already used for a different payment operation' \
  "$TMP_DIR/payment-authorize-conflict.err" || \
  fail "Payment gRPC changed-fingerprint replay did not return ALREADY_EXISTS"
echo "Payment gRPC replay returned durable responses and rejected a changed fingerprint"

assert_postgres() {
  local label="$1" query="$2" result
  if ! result="$(psql_checkout_query "$query" 2>"$TMP_DIR/postgres-$label.err")"; then
    fail "PostgreSQL $label query failed"
  fi
  result="$(tr -d '[:space:]' <<<"$result")"
  [[ "$result" == 1 ]] || fail "PostgreSQL $label assertion failed"
}

assert_postgres order "
  SELECT (count(*) = 1)::int
    FROM orders
   WHERE tenant_id = :'tenant_id'
     AND id = :'order_id'
     AND customer_id = :'customer_id'
     AND currency = :'expected_currency'
     AND subtotal_minor = (:'expected_subtotal_minor')::integer
     AND discount_minor = 0
     AND tax_minor = 0
     AND shipping_minor = 0
     AND total_minor = (:'expected_total_minor')::integer
     AND status IN ('paid', 'processing', 'shipped', 'delivered');
"
assert_postgres cart "
  SELECT (count(*) = 1)::int
    FROM carts c
    JOIN orders o ON o.tenant_id = c.tenant_id AND o.cart_id = c.id
   WHERE o.tenant_id = :'tenant_id'
     AND o.id = :'order_id'
     AND o.cart_id = :'cart_id'
     AND c.status = 'checked_out';
"
assert_postgres order-items "
  SELECT (count(*) = 1)::int
    FROM order_items
   WHERE tenant_id = :'tenant_id'
     AND order_id = :'order_id'
     AND sku = :'sku'
     AND quantity = 1
     AND unit_price_minor = (:'expected_subtotal_minor')::integer
     AND discount_minor = 0
     AND line_total_minor = (:'expected_total_minor')::integer;
"
assert_postgres payment "
  SELECT (count(*) = 1)::int
    FROM payments
   WHERE tenant_id = :'tenant_id'
     AND order_id = :'order_id'
     AND status = 'captured'
     AND currency = :'expected_currency'
     AND amount_minor = (:'expected_total_minor')::integer
     AND captured_at IS NOT NULL
     AND captured_amount_minor = (:'expected_total_minor')::integer;
"
assert_postgres payment-authorize-operation "
  SELECT (count(*) = 1)::int
    FROM payment_operation_requests
   WHERE tenant_id = :'tenant_id'
     AND request_id = (:'request_id' || ':authorize')
     AND operation = 'authorize'
     AND payment_id IS NOT NULL
     AND response_payload IS NOT NULL
     AND grpc_status IS NULL
     AND operation_amount_minor = (:'expected_total_minor')::bigint;
"
assert_postgres payment-capture-operation "
  SELECT (count(*) = 1)::int
    FROM payment_operation_requests
   WHERE tenant_id = :'tenant_id'
     AND request_id = (:'request_id' || ':capture')
     AND operation = 'capture'
     AND payment_id IS NOT NULL
     AND response_payload IS NOT NULL
     AND grpc_status IS NULL
     AND operation_amount_minor = (:'expected_total_minor')::bigint;
"
assert_postgres checkout-attempt "
  SELECT (count(*) = 1)::int
   FROM checkout_attempts
   WHERE tenant_id = :'tenant_id'
     AND request_id = :'request_id'
     AND order_id = :'order_id'
     AND status = 'paid'
     AND response_payload IS NOT NULL;
"
assert_postgres decline-attempt "
  SELECT (count(*) = 1)::int
    FROM checkout_attempts
   WHERE tenant_id = :'tenant_id'
     AND request_id = :'decline_request_id'
     AND status = 'failed'
     AND error_status = 402
     AND error_code = 'payment_declined'
     AND response_payload IS NULL;
"
assert_postgres decline-payment-operation "
  SELECT (count(*) = 1)::int
    FROM payment_operation_requests
   WHERE tenant_id = :'tenant_id'
     AND request_id = (:'decline_request_id' || ':authorize')
     AND operation = 'authorize'
     AND payment_id IS NOT NULL
     AND response_payload IS NOT NULL
     AND grpc_status IS NULL
     AND operation_amount_minor = (:'expected_total_minor')::bigint;
"
assert_postgres decline-payment "
  SELECT (count(*) = 1)::int
    FROM payment_operation_requests r
    JOIN payments p ON p.tenant_id = r.tenant_id AND p.id = r.payment_id
   WHERE r.tenant_id = :'tenant_id'
     AND r.request_id = (:'decline_request_id' || ':authorize')
     AND p.status = 'failed'
     AND p.failure_code = 'payment_failure_reason_declined'
     AND p.currency = :'expected_currency'
     AND p.amount_minor = (:'expected_total_minor')::integer;
"
assert_postgres outbox "
  SELECT (count(*) = 1)::int
    FROM outbox_events
   WHERE tenant_id = :'tenant_id'
     AND event_key = :'event_key'
     AND aggregate_id = :'order_id'
     AND event_type = 'order.paid'
     AND aggregate_type = 'order'
     AND schema_version = 1
     AND occurred_at IS NOT NULL
     AND status = 'published'
     AND published_at IS NOT NULL
     AND traceparent LIKE ('00-' || :'trace_id' || '-%')
     AND length(traceparent) = 55
     AND tracestate = :'tracestate'
     AND baggage LIKE ('%tenant.id=' || :'tenant_id' || '%')
     AND baggage LIKE '%customer.segment=standard%'
     AND baggage LIKE '%region=us-east-1%'
     AND baggage LIKE '%request.priority=normal%'
     AND baggage LIKE ('%session.id=' || :'session_id' || '%')
     AND baggage LIKE ('%feature.variant=' || :'feature_variant' || '%')
     AND payload->>'order_id' = :'order_id'
     AND payload->>'tenant_id' = :'tenant_id';
"
assert_postgres analytics "
  SELECT (count(*) = 1)::int
    FROM analytics_events
   WHERE tenant_id = :'tenant_id'
     AND event_key = :'event_key'
     AND customer_id = :'customer_id'
     AND source = 'checkout'
     AND event_name = 'order.paid'
     AND entity_type = 'order'
     AND entity_id = :'order_id'
     AND session_id = :'session_id'
     AND occurred_at IS NOT NULL
     AND length(trace_id) = 32
     AND trace_id = :'trace_id'
     AND context->>'feature_variant' = :'feature_variant'
     AND context->>'customer_segment' = 'standard'
     AND context->>'region' = 'us-east-1'
     AND context->>'request_priority' = 'normal';
"
assert_postgres exposure "
  SELECT (count(*) = 1)::int
    FROM feature_exposures
   WHERE tenant_id = :'tenant_id'
     AND customer_id = :'customer_id'
     AND feature_key = 'checkoutFlow'
     AND variant = :'feature_variant'
     AND exposure_key = (:'event_key' || ':checkoutFlow')
     AND session_id = :'session_id'
     AND context->>'feature_variant' = :'feature_variant'
     AND context->>'customer_segment' = 'standard'
     AND context->>'region' = 'us-east-1'
     AND context->>'request_priority' = 'normal';
"
assert_postgres shipment "
  SELECT (count(*) = 1)::int
   FROM shipments
   WHERE tenant_id = :'tenant_id'
     AND order_id = :'order_id'
     AND id = :'shipment_id'
     AND shipment_number = 1
     AND status = 'label_created';
"
assert_postgres fulfillment-claim "
  SELECT (count(*) = 1)::int
   FROM fulfillment_processed_events
   WHERE tenant_id = :'tenant_id'
     AND order_id = :'order_id'
     AND consumer_name = 'fulfillment.orders'
     AND event_key = :'fulfillment_claim_key'
     AND status = 'completed'
     AND attempts BETWEEN 1 AND 5;
"
assert_postgres analytics-claim "
  SELECT (count(*) = 1)::int
    FROM fulfillment_processed_events
   WHERE tenant_id = :'tenant_id'
     AND order_id = :'order_id'
     AND consumer_name = 'fulfillment.analytics'
     AND event_key = :'event_key'
     AND status = 'completed'
     AND attempts BETWEEN 1 AND 5;
"
assert_postgres notification "
  SELECT (count(*) = 1)::int
    FROM notification_deliveries
   WHERE tenant_id = :'tenant_id'
     AND order_id = :'order_id'
     AND status = 'delivered'
     AND event_key = (:'fulfillment_claim_key' || ':notification:label_created')
     AND channel = 'in_app'
     AND payload->>'event_type' = 'order.paid'
     AND payload->>'fulfillment_status' = 'label_created'
     AND payload->>'customer_id' = :'customer_id'
     AND traceparent ~ '^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$'
     AND tracestate LIKE '%=%'
     AND baggage LIKE ('%tenant.id=' || :'tenant_id' || '%')
     AND attempts >= 1
     AND delivered_at IS NOT NULL
     AND EXISTS (
           SELECT 1
             FROM notification_channel_messages m
            WHERE m.tenant_id = notification_deliveries.tenant_id
              AND m.delivery_id = notification_deliveries.id
              AND m.channel = 'in_app'
              AND length(trim(m.acknowledgement_reference)) > 0
         );
"
echo "PostgreSQL order, payment, cart, outbox, analytics, exposure, claim, shipment, and notification assertions passed"

assert_redis_exists() {
  local label="$1" key="$2" value
  value="$(redis_cli EXISTS "$key" 2>"$TMP_DIR/redis-$label.err" || true)"
  [[ "$(tr -d '[:space:]' <<<"$value")" == 1 ]] || fail "Redis $label assertion failed"
}

pricing_cache_keys="$(redis_cli --scan --pattern "$pricing_cache_pattern" \
  2>"$TMP_DIR/pricing-cache-assert.err" || true)"
[[ -n "$pricing_cache_keys" ]] || fail "Redis pricing quote cache assertion failed"
if [[ "$MANAGE_STACK" == 1 ]]; then
  pricing_cache_key_count="$(awk 'NF {count++} END {print count + 0}' <<<"$pricing_cache_keys")"
  [[ "$pricing_cache_key_count" -ge 2 ]] || \
    fail "checkout did not retain both scoped pricing quote keys (got $pricing_cache_key_count)"
  echo "Redis catalog and pricing cache assertions passed"
else
  echo "Redis catalog and pricing cache assertions passed (external state retained)"
fi

assert_rabbit_queue() {
  local queue="$1" body expected_retry_route
  body="$(rabbit_queue_json "$queue" 2>"$TMP_DIR/rabbit-${queue}.err" || true)"
  [[ -n "$body" ]] || fail "RabbitMQ queue is missing: $queue"
  jq -e --arg queue "$queue" '.name == $queue and .durable == true and .auto_delete == false' <<<"$body" \
    >/dev/null 2>&1 || fail "RabbitMQ queue is not durable: $queue"
  case "$queue" in
    *.dead)
      # RabbitMQ 4 reports its broker-selected classic queue type even when
      # QueueBuilder declared no custom dead-letter arguments.
      jq -e '(.arguments // {}) == {"x-queue-type":"classic"}' <<<"$body" >/dev/null 2>&1 || \
        fail "RabbitMQ dead-letter queue has unexpected arguments: $queue"
      ;;
    *.retry)
      expected_retry_route="$queue"
      [[ "$queue" == "$ANALYTICS_QUEUE.retry" ]] && expected_retry_route="$ANALYTICS_RETRY_ROUTING_KEY"
      jq -e --arg route "$expected_retry_route" \
        '.arguments["x-message-ttl"] > 0
             and .arguments["x-dead-letter-exchange"] == "commerce.retry.return"
             and .arguments["x-dead-letter-routing-key"] == $route' \
        <<<"$body" \
        >/dev/null 2>&1 || fail "RabbitMQ retry queue arguments are unsafe: $queue"
      ;;
    *)
      jq -e '.arguments["x-dead-letter-exchange"] == "commerce.dlx"
             and .arguments["x-dead-letter-routing-key"] == (.name + ".dead")' <<<"$body" \
        >/dev/null 2>&1 || fail "RabbitMQ queue dead-letter policy is missing: $queue"
      ;;
  esac
}

assert_rabbit_binding() {
  local queue="$1" source="$2" routing_key="$3" body
  body="$(rabbit_queue_bindings_json "$queue" 2>"$TMP_DIR/rabbit-${queue}-bindings.err")" || \
    fail "RabbitMQ bindings could not be inspected: $queue"
  jq -e --arg queue "$queue" --arg source "$source" --arg routing_key "$routing_key" \
    'any(.[]; .destination == $queue and .source == $source and .routing_key == $routing_key)' <<<"$body" \
    >/dev/null 2>&1 || fail "RabbitMQ binding is missing: $source -> $queue ($routing_key)"
}

ANALYTICS_RETRY_ROUTING_KEY="${ANALYTICS_QUEUE}.retry"

for rabbit_queue in \
  "$FULFILLMENT_QUEUE" "${FULFILLMENT_QUEUE}.dead" "${FULFILLMENT_QUEUE}.retry" \
  "$ANALYTICS_QUEUE" "${ANALYTICS_QUEUE}.dead" "${ANALYTICS_QUEUE}.retry"; do
  assert_rabbit_queue "$rabbit_queue"
done
assert_rabbit_binding "$FULFILLMENT_QUEUE" commerce.events 'order.#'
assert_rabbit_binding "$FULFILLMENT_QUEUE" commerce.events 'payment.#'
assert_rabbit_binding "$FULFILLMENT_QUEUE" commerce.retry.return "${FULFILLMENT_QUEUE}.retry"
assert_rabbit_binding "${FULFILLMENT_QUEUE}.retry" commerce.retry "${FULFILLMENT_QUEUE}.retry"
assert_rabbit_binding "${FULFILLMENT_QUEUE}.dead" commerce.dlx "${FULFILLMENT_QUEUE}.dead"
assert_rabbit_binding "$ANALYTICS_QUEUE" commerce.events '#'
assert_rabbit_binding "$ANALYTICS_QUEUE" commerce.retry.return "$ANALYTICS_RETRY_ROUTING_KEY"
assert_rabbit_binding "${ANALYTICS_QUEUE}.retry" commerce.retry "$ANALYTICS_RETRY_ROUTING_KEY"
assert_rabbit_binding "${ANALYTICS_QUEUE}.dead" commerce.dlx "${ANALYTICS_QUEUE}.dead"
echo "RabbitMQ durable fulfillment, analytics, retry, and dead-letter queues passed"

if [[ "$MANAGE_STACK" == 1 ]]; then
  # RabbitMQ is downstream of the checkout transaction. Stop it before the
  # write, prove the API still commits a durable outbox event, then recover the
  # broker and require publication plus fulfillment completion.
  RABBIT_OUTAGE_REQUEST_ID="commerce-verify-rabbit-outage-${RUN_ID}"
  RABBIT_OUTAGE_SESSION_ID="session-${RUN_ID}-rabbit-outage"
  [[ "$RABBIT_OUTAGE_REQUEST_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
    fail "generated RabbitMQ outage request id is unsafe"
  [[ "$RABBIT_OUTAGE_SESSION_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
    fail "generated RabbitMQ outage session id is unsafe"

  run_bounded_seconds rabbit-outage-stop "$FAILURE_PROBE_TIMEOUT" \
    "${COMPOSE[@]}" stop rabbitmq || fail "could not stop RabbitMQ for outbox isolation proof"
  wait_for_container_state rabbitmq exited "$FAILURE_PROBE_TIMEOUT" || \
    fail "RabbitMQ did not stop before the isolated checkout write"

  rabbit_outage_health_deadline=$((SECONDS + FAILURE_PROBE_TIMEOUT))
  rabbit_outage_core_ready=0
  while (( SECONDS < rabbit_outage_health_deadline )); do
    rabbit_outage_health_code="$(curl -sS --max-time "$FAILURE_HTTP_TIMEOUT" \
      -o "$TMP_DIR/checkout-rabbit-outage-health.json" -w '%{http_code}' \
      "$CHECKOUT_URL/healthz" 2>"$TMP_DIR/checkout-rabbit-outage-health.curl.log" || true)"
    if [[ "$rabbit_outage_health_code" == 200 ]] && jq -e \
      '.status == "UP" and .messaging == "DOWN"' \
      "$TMP_DIR/checkout-rabbit-outage-health.json" >/dev/null 2>&1; then
      rabbit_outage_core_ready=1
      break
    fi
    sleep 1
  done
  (( rabbit_outage_core_ready == 1 )) || \
    fail "checkout core health did not remain UP while RabbitMQ was stopped"

  rabbit_outage_response="$TMP_DIR/checkout-rabbit-outage.json"
  send_checkout "$RABBIT_OUTAGE_REQUEST_ID" "$rabbit_outage_response" 0 tok_visa 1 \
    "$RABBIT_OUTAGE_SESSION_ID"
  [[ "$CHECKOUT_HTTP_CODE" == 200 ]] || \
    fail "checkout write failed while RabbitMQ was stopped (HTTP $CHECKOUT_HTTP_CODE)"
  jq -e '.status == "paid" and .payment_status == "captured"' \
    "$rabbit_outage_response" >/dev/null 2>&1 || \
    fail "checkout did not commit a paid order while RabbitMQ was stopped"
  RABBIT_OUTAGE_ORDER_ID="$(jq -r '.order_id // empty' "$rabbit_outage_response")"
  [[ "$RABBIT_OUTAGE_ORDER_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
    fail "RabbitMQ outage checkout returned an unsafe order id"
  RABBIT_OUTAGE_EVENT_KEY="${RABBIT_OUTAGE_ORDER_ID}:paid"
  rabbit_outage_outbox_state="$(psql_query "
    SELECT status || ':' || CASE WHEN published_at IS NULL THEN 'unpublished' ELSE 'published' END
      FROM outbox_events
     WHERE tenant_id = '${TENANT_ID}'
       AND event_key = '${RABBIT_OUTAGE_EVENT_KEY}';
  " 2>"$TMP_DIR/rabbit-outage-outbox-before-recovery.err" || true)"
  rabbit_outage_outbox_state="$(tr -d '[:space:]' <<<"$rabbit_outage_outbox_state")"
  [[ "$rabbit_outage_outbox_state" =~ ^(queued|processing):unpublished$ ]] || \
    fail "checkout did not leave an unpublished durable outbox event during RabbitMQ outage (state=$rabbit_outage_outbox_state)"
  echo "RabbitMQ outage write passed: checkout stayed available and durable outbox remained unpublished"

  run_bounded_seconds rabbit-outage-start "$FAILURE_PROBE_TIMEOUT" \
    "${COMPOSE[@]}" start rabbitmq || fail "could not restart RabbitMQ after outbox isolation proof"
  wait_for_healthy rabbitmq "$FAILURE_PROBE_TIMEOUT" || \
    fail "RabbitMQ did not recover after outbox isolation proof"
  wait_for_healthy fulfillment "$FAILURE_PROBE_TIMEOUT" || \
    fail "fulfillment did not recover after RabbitMQ restart"

  rabbit_outage_recovery_deadline=$((SECONDS + ASYNC_TIMEOUT))
  rabbit_outage_published=0
  rabbit_outage_fulfillment_ready=0
  while (( SECONDS < rabbit_outage_recovery_deadline )); do
    rabbit_outage_outbox_state="$(psql_query "
      SELECT status || ':' || CASE WHEN published_at IS NULL THEN 'unpublished' ELSE 'published' END
        FROM outbox_events
       WHERE tenant_id = '${TENANT_ID}'
         AND event_key = '${RABBIT_OUTAGE_EVENT_KEY}';
    " 2>"$TMP_DIR/rabbit-outage-outbox-after-recovery.err" || true)"
    rabbit_outage_outbox_state="$(tr -d '[:space:]' <<<"$rabbit_outage_outbox_state")"
    [[ "$rabbit_outage_outbox_state" == published:published ]] && rabbit_outage_published=1

    rabbit_outage_fulfillment_code="$(curl -sS --max-time "$FAILURE_HTTP_TIMEOUT" \
      -o "$TMP_DIR/fulfillment-rabbit-outage.json" -w '%{http_code}' \
      "${FULFILLMENT_AUTH_HEADERS[@]}" \
      "${FULFILLMENT_URL%/}/verify/order?order=$RABBIT_OUTAGE_ORDER_ID" \
      2>"$TMP_DIR/fulfillment-rabbit-outage.curl.log" || true)"
    if [[ "$rabbit_outage_fulfillment_code" == 200 ]] && jq -e \
      '.ready == true and .fulfillment_status == "completed"
       and (.shipment_id | type == "string" and length > 0)' \
      "$TMP_DIR/fulfillment-rabbit-outage.json" >/dev/null 2>&1; then
      rabbit_outage_fulfillment_ready=1
    fi
    if (( rabbit_outage_published == 1 && rabbit_outage_fulfillment_ready == 1 )); then
      break
    fi
    sleep 1
  done
  (( rabbit_outage_published == 1 )) || \
    fail "durable outbox event did not publish after RabbitMQ recovery"
  (( rabbit_outage_fulfillment_ready == 1 )) || \
    fail "fulfillment did not complete the RabbitMQ outage order after recovery"
  echo "RabbitMQ outage recovery passed: durable outbox published and async fulfillment completed"
else
  echo "RabbitMQ outage isolation skipped in external mode"
fi

PRIMARY_TRACE_ID="${PRIMARY_TRACEPARENT:3:32}"
clickhouse_order_query="SELECT count() FROM analytics.analytics_events FINAL WHERE tenant_id = '${TENANT_ID}' AND event_key = '${ORDER_EVENT_KEY}' AND event_name = 'order.paid' AND entity_type = 'order' AND entity_id = '${ORDER_ID}' AND session_id = '${PRIMARY_SESSION_ID}' AND length(trace_id) = 32 AND trace_id = '${PRIMARY_TRACE_ID}' AND length(traceparent) = 55 AND traceparent LIKE '00-${PRIMARY_TRACE_ID}-%' AND tracestate = '${PRIMARY_TRACESTATE}' AND position(baggage, 'session.id=${PRIMARY_SESSION_ID}') > 0 AND feature_variant = '${CHECKOUT_FEATURE_VARIANT}' AND properties != '{}' AND context != '{}'"
clickhouse_exposure_query="SELECT count() FROM analytics.feature_exposures FINAL WHERE tenant_id = '${TENANT_ID}' AND exposure_key = '${ORDER_EVENT_KEY}:checkoutFlow' AND customer_id = '${CUSTOMER_ID}' AND session_id = '${PRIMARY_SESSION_ID}' AND feature_key = 'checkoutFlow' AND variant = '${CHECKOUT_FEATURE_VARIANT}'"
clickhouse_deadline=$((SECONDS + ASYNC_TIMEOUT))
clickhouse_ready=0
while (( SECONDS < clickhouse_deadline )); do
  clickhouse_count="$(clickhouse_query "$clickhouse_order_query" 2>"$TMP_DIR/clickhouse-order.err" || true)"
  clickhouse_count="$(tr -d '[:space:]' <<<"$clickhouse_count")"
  exposure_count="$(clickhouse_query "$clickhouse_exposure_query" 2>"$TMP_DIR/clickhouse-exposure.err" || true)"
  exposure_count="$(tr -d '[:space:]' <<<"$exposure_count")"
  if [[ "$clickhouse_count" == 1 && "$exposure_count" == 1 ]]; then
    clickhouse_ready=1
    break
  fi
  sleep 1
done
(( clickhouse_ready == 1 )) || fail "ClickHouse analytics and feature-exposure records did not arrive within ${ASYNC_TIMEOUT}s"
echo "ClickHouse analytics and feature-exposure assertions passed"

storefront_analytics_payload="$(jq -cn --arg tenant "$TENANT_ID" --arg event "$ORDER_EVENT_KEY" \
  '{
    operationName: "StorefrontAnalytics",
    query: "query StorefrontAnalytics($tenantId: String, $eventName: String) { analyticsEvents(tenantId: $tenantId, eventName: $eventName, limit: 100) { tenantId eventKey customerId eventName entityType entityId occurredAt traceId traceparent tracestate baggage featureVariant properties context } analyticsSummary(tenantId: $tenantId, eventName: $eventName) { eventCount uniqueCustomers } }",
    variables: {tenantId: $tenant, eventName: "order.paid"}
  }')"
storefront_analytics_response="$TMP_DIR/storefront-analytics.json"
graphql_post storefront-analytics "$STOREFRONT_GRAPHQL_URL" "$storefront_analytics_payload" "$storefront_analytics_response"
jq -e --arg tenant "$TENANT_ID" --arg event "$ORDER_EVENT_KEY" --arg customer "$CUSTOMER_ID" \
  --arg trace_id "$PRIMARY_TRACE_ID" --arg session "$PRIMARY_SESSION_ID" --arg variant "$CHECKOUT_FEATURE_VARIANT" \
  'any(.data.analyticsEvents[]?;
      .tenantId == $tenant
      and .eventKey == $event
      and .customerId == $customer
      and .eventName == "order.paid"
      and .entityType == "order"
      and .entityId == ($event | sub(":paid$"; ""))
      and .traceId == $trace_id
      and (.traceparent | startswith("00-" + $trace_id + "-"))
      and (.traceparent | length == 55)
      and .tracestate == "playground=commerce"
      and (.baggage | contains("session.id=" + $session))
      and .featureVariant == $variant)
   and .data.analyticsSummary.eventCount >= 1
   and .data.analyticsSummary.uniqueCustomers >= 1' \
  "$storefront_analytics_response" >/dev/null 2>&1 || fail "Storefront analytics query did not expose the durable event"
echo "Storefront analytics query passed"

if [[ "$MANAGE_STACK" == 1 ]]; then
  # ClickHouse is an analytics sink. A sink outage must not stop the durable
  # order/fulfillment consumer, while the analytics delivery remains visible in
  # its durable retry/dead-letter path for operator redrive.
  CLICKHOUSE_OUTAGE_REQUEST_ID="commerce-verify-clickhouse-outage-${RUN_ID}"
  CLICKHOUSE_OUTAGE_SESSION_ID="session-${RUN_ID}-clickhouse-outage"
  [[ "$CLICKHOUSE_OUTAGE_REQUEST_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
    fail "generated ClickHouse outage request id is unsafe"
  [[ "$CLICKHOUSE_OUTAGE_SESSION_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
    fail "generated ClickHouse outage session id is unsafe"
  analytics_dead_before="$(rabbit_queue_json_bounded "${ANALYTICS_QUEUE}.dead" \
    | jq -r '.messages_ready // 0' 2>"$TMP_DIR/clickhouse-outage-queue-before.err")" || \
    fail "could not inspect the analytics dead-letter queue before ClickHouse outage"
  [[ "$analytics_dead_before" =~ ^[0-9]+$ ]] || \
    fail "analytics dead-letter queue returned an invalid message count before outage"

  run_bounded_seconds clickhouse-outage-stop "$FAILURE_PROBE_TIMEOUT" \
    "${COMPOSE[@]}" stop clickhouse || fail "could not stop ClickHouse for isolation proof"
  wait_for_healthy fulfillment "$FAILURE_PROBE_TIMEOUT" || \
    fail "fulfillment lost readiness when ClickHouse stopped"

  clickhouse_outage_response="$TMP_DIR/checkout-clickhouse-outage.json"
  send_checkout "$CLICKHOUSE_OUTAGE_REQUEST_ID" "$clickhouse_outage_response" 0 tok_visa 1 \
    "$CLICKHOUSE_OUTAGE_SESSION_ID"
  [[ "$CHECKOUT_HTTP_CODE" == 200 ]] || \
    fail "checkout was unavailable while ClickHouse was stopped (HTTP $CHECKOUT_HTTP_CODE)"
  jq -e '.status == "paid" and .payment_status == "captured"' \
    "$clickhouse_outage_response" >/dev/null 2>&1 || \
    fail "checkout did not commit an order while ClickHouse was stopped"
  CLICKHOUSE_OUTAGE_ORDER_ID="$(jq -r '.order_id // empty' "$clickhouse_outage_response")"
  [[ "$CLICKHOUSE_OUTAGE_ORDER_ID" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
    fail "ClickHouse outage checkout returned an unsafe order id"
  CLICKHOUSE_OUTAGE_EVENT_KEY="${CLICKHOUSE_OUTAGE_ORDER_ID}:paid"

  clickhouse_outage_deadline=$((SECONDS + ASYNC_TIMEOUT))
  clickhouse_outage_fulfillment_ready=0
  clickhouse_outage_analytics_dead=0
  while (( SECONDS < clickhouse_outage_deadline )); do
    clickhouse_outage_fulfillment_code="$(curl -sS --max-time "$FAILURE_HTTP_TIMEOUT" \
      -o "$TMP_DIR/fulfillment-clickhouse-outage.json" -w '%{http_code}' \
      "${FULFILLMENT_AUTH_HEADERS[@]}" \
      "${FULFILLMENT_URL%/}/verify/order?order=$CLICKHOUSE_OUTAGE_ORDER_ID" \
      2>"$TMP_DIR/fulfillment-clickhouse-outage.curl.log" || true)"
    if [[ "$clickhouse_outage_fulfillment_code" == 200 ]] && jq -e \
      '.ready == true and .fulfillment_status == "completed"
       and (.shipment_id | type == "string" and length > 0)' \
      "$TMP_DIR/fulfillment-clickhouse-outage.json" >/dev/null 2>&1; then
      clickhouse_outage_fulfillment_ready=1
    fi

    analytics_dead_after="$(rabbit_queue_json_bounded "${ANALYTICS_QUEUE}.dead" \
      | jq -r '.messages_ready // 0' 2>"$TMP_DIR/clickhouse-outage-queue-after.err" || true)"
    analytics_claim_status="$(psql_query "
      SELECT status
        FROM fulfillment_processed_events
       WHERE tenant_id = '${TENANT_ID}'
         AND order_id = '${CLICKHOUSE_OUTAGE_ORDER_ID}'
         AND consumer_name = 'fulfillment.analytics'
         AND event_key = '${CLICKHOUSE_OUTAGE_EVENT_KEY}'
       ORDER BY claimed_at DESC
       LIMIT 1;
    " 2>"$TMP_DIR/clickhouse-outage-claim.err" || true)"
    analytics_claim_status="$(tr -d '[:space:]' <<<"$analytics_claim_status")"
    if [[ "$analytics_dead_after" =~ ^[0-9]+$ ]] \
      && (( analytics_dead_after > analytics_dead_before )) \
      && [[ "$analytics_claim_status" == dead_lettered ]]; then
      clickhouse_outage_analytics_dead=1
    fi
    if (( clickhouse_outage_fulfillment_ready == 1 && clickhouse_outage_analytics_dead == 1 )); then
      break
    fi
    sleep 1
  done
  (( clickhouse_outage_fulfillment_ready == 1 )) || \
    fail "fulfillment did not complete the order while ClickHouse was unavailable"
  (( clickhouse_outage_analytics_dead == 1 )) || \
    fail "analytics failure did not reach its durable dead-letter/retry boundary"

  run_bounded_seconds clickhouse-outage-start "$FAILURE_PROBE_TIMEOUT" \
    "${COMPOSE[@]}" start clickhouse || fail "could not restart ClickHouse after isolation proof"
  wait_for_healthy clickhouse "$FAILURE_PROBE_TIMEOUT" || \
    fail "ClickHouse did not recover after isolation proof"
  wait_for_healthy fulfillment "$FAILURE_PROBE_TIMEOUT" || \
    fail "fulfillment did not remain ready after ClickHouse recovery"
  analytics_dead_after_recovery="$(rabbit_queue_json_bounded "${ANALYTICS_QUEUE}.dead" \
    | jq -r '.messages_ready // 0' 2>"$TMP_DIR/clickhouse-outage-queue-recovery.err")" || \
    fail "could not inspect the analytics dead-letter queue after ClickHouse recovery"
  if [[ ! "$analytics_dead_after_recovery" =~ ^[0-9]+$ ]] \
    || (( analytics_dead_after_recovery <= analytics_dead_before )); then
    fail "analytics dead-letter record was not retained after ClickHouse recovery"
  fi
  echo "ClickHouse outage isolation passed: fulfillment completed; analytics retry/dead-letter record retained"
else
  echo "ClickHouse outage isolation skipped in external mode"
fi

if [[ "$MANAGE_STACK" == 1 ]]; then
  catalog_page_cache_pattern="catalog:v2:page:${catalog_tenant_key}:*"
  catalog_page_cache_keys="$(redis_cli --scan --pattern "$catalog_page_cache_pattern" 2>"$TMP_DIR/catalog-page-cache-clear.err")" || \
    fail "could not inspect the browser catalog cache"
  if [[ -n "$catalog_page_cache_keys" ]]; then
    while IFS= read -r catalog_page_cache_key; do
      [[ -n "$catalog_page_cache_key" ]] || continue
      redis_cli DEL "$catalog_page_cache_key" >/dev/null || fail "could not clear the browser catalog cache"
    done <<<"$catalog_page_cache_keys"
  fi
  catalog_page_cache_after_clear="$(redis_cli --scan --pattern "$catalog_page_cache_pattern" 2>"$TMP_DIR/catalog-page-cache-after-clear.err")" || \
    fail "could not verify the browser catalog cache reset"
  [[ -z "$(awk 'NF {print; exit}' <<<"$catalog_page_cache_after_clear")" ]] || \
    fail "browser catalog cache retained a scoped key after reset"
  run_bounded browser-e2e env PLAYGROUND_COMPOSE_BASE_URL="$WEB_URL" \
    bun --cwd "$ROOT/web" run e2e:compose || fail "Compose-backed browser commerce journey failed"
  echo "Compose-backed browser commerce journey passed"
else
  echo "Compose-backed browser commerce journey skipped in external mode"
fi

if [[ "$MANAGE_STACK" == 1 ]]; then
  checkout_container_before="$(container_id checkout)" || fail "checkout container identity is unavailable"
  FLIP_VARIANT=control
  [[ "$EXPECTED_CHECKOUT_VARIANT" == control ]] && FLIP_VARIANT=orchestrated
  flip_flag() {
    local variant="$1"
    jq -e \
      '.flags.checkoutFlow.variants.control and .flags.checkoutFlow.variants.orchestrated' \
      "$FLAG_CONFIG" >/dev/null || fail "checkout feature variants are missing"
    jq --arg variant "$variant" \
      '.flags.checkoutFlow.defaultVariant = $variant' \
      "$FLAG_CONFIG" >"$TMP_DIR/flagd-next.json" || fail "could not prepare checkout feature-variant flip"
    if ! cp "$TMP_DIR/flagd-next.json" "$FLAG_CONFIG"; then
      fail "could not apply checkout feature-variant flip"
    fi
    # Recreate only flagd so the bind-mounted file is reopened deterministically;
    # the checkout container must remain the same process.
    run_bounded flagd-reload "${COMPOSE[@]}" up -d --no-deps --force-recreate flagd || \
      fail "could not reload flagd after feature-variant flip"
    wait_for_healthy flagd || fail "flagd did not recover after feature-variant flip"
  }
  wait_for_checkout_variant() {
    local expected="$1" prefix="$2" deadline=$((SECONDS + ASYNC_TIMEOUT)) attempt=0 response="$TMP_DIR/checkout-flip.json"
    while (( SECONDS < deadline )); do
      local checkout_session_id="${prefix}-session-${attempt}"
      [[ "$checkout_session_id" =~ ^[A-Za-z0-9._-]{1,128}$ ]] || \
        fail "generated feature-flip session id is unsafe"
      send_checkout "${prefix}-${attempt}" "$response" 0 tok_visa 1 "$checkout_session_id"
      if [[ "$CHECKOUT_HTTP_CODE" == 200 ]] && jq -e --arg expected "$expected" --arg tenant "$TENANT_ID" \
        '.status == "paid"
         and .payment_status == "captured"
         and .feature_variant == $expected
         and (if $expected == "control" then .recommendation == null
              else (.recommendation.source == "catalog-graphql"
                    and .recommendation.tenant_id == $tenant
                    and (.recommendation.recommended | type == "array")) end)' \
        "$response" >/dev/null 2>&1; then
        return 0
      fi
      attempt=$((attempt + 1))
      sleep 1
    done
    local last_response
    last_response="$(jq -c '{error,message,status,payment_status,feature_variant,recommendation: (if .recommendation == null then null else {source: .recommendation.source, tenant_id: .recommendation.tenant_id, recommended: .recommendation.recommended} end)}' "$response" 2>/dev/null || true)"
    fail "checkout feature variant did not flip to $expected within ${ASYNC_TIMEOUT}s (http=${CHECKOUT_HTTP_CODE:-unknown} response=${last_response:-invalid-json})"
  }
  flip_flag "$FLIP_VARIANT"
  wait_for_checkout_variant "$FLIP_VARIANT" "commerce-verify-flip-${RUN_ID}"
  checkout_container_after="$(container_id checkout)" || fail "checkout container identity disappeared after feature flip"
  [[ "$checkout_container_before" == "$checkout_container_after" ]] || \
    fail "checkout restarted during feature-variant flip"

  run_bounded flagd-outage-stop "${COMPOSE[@]}" stop flagd || fail "could not stop flagd for fallback proof"
  wait_for_healthy checkout || fail "checkout health changed when optional flagd was stopped"
  wait_for_checkout_variant orchestrated "commerce-verify-flagd-outage-${RUN_ID}"
  run_bounded flagd-outage-recover "${COMPOSE[@]}" up -d --no-deps --force-recreate flagd || \
    fail "could not recreate flagd after fallback proof"
  wait_for_healthy flagd || fail "flagd did not recover after fallback proof"
  flip_flag "$EXPECTED_CHECKOUT_VARIANT"
  wait_for_checkout_variant "$EXPECTED_CHECKOUT_VARIANT" "commerce-verify-restore-${RUN_ID}"
  checkout_container_after="$(container_id checkout)" || fail "checkout container identity disappeared after feature-variant restore"
  [[ "$checkout_container_before" == "$checkout_container_after" ]] || \
    fail "checkout restarted during feature-variant restore"
  echo "Feature-variant flip passed without checkout restart"
else
  echo "Feature-variant flip skipped in external mode"
fi

if [[ "$MANAGE_STACK" == 1 ]]; then
  checkout_container_before="${checkout_container_before:-$(container_id checkout)}"
  run_bounded readiness-stop "${COMPOSE[@]}" stop catalog || fail "could not stop catalog for readiness failure check"
  readiness_deadline=$((SECONDS + ASYNC_TIMEOUT))
  readiness_failed=0
  while (( SECONDS < readiness_deadline )); do
    readiness_code="$(curl -sS --max-time "$HTTP_TIMEOUT" -o "$TMP_DIR/checkout-readiness-failure.json" \
      -w '%{http_code}' "$CHECKOUT_URL/healthz" 2>"$TMP_DIR/checkout-readiness-failure.curl.log" || true)"
    if [[ "$readiness_code" == 503 ]]; then
      readiness_failed=1
      break
    fi
    sleep 1
  done
  (( readiness_failed == 1 )) || fail "checkout readiness did not fail when catalog was stopped"
  run_bounded readiness-start "${COMPOSE[@]}" start catalog || fail "could not restart catalog for readiness recovery check"
  wait_for_healthy catalog || fail "catalog did not recover for readiness check"
  wait_for_healthy checkout || fail "checkout did not recover for readiness check"
  readiness_code="$(curl -sS --max-time "$HTTP_TIMEOUT" -o "$TMP_DIR/checkout-readiness-recovery.json" \
    -w '%{http_code}' "$CHECKOUT_URL/healthz" 2>"$TMP_DIR/checkout-readiness-recovery.curl.log" || true)"
  [[ "$readiness_code" == 200 ]] || fail "checkout readiness did not recover after catalog restart"
  checkout_container_after="$(container_id checkout)" || fail "checkout container identity unavailable after readiness recovery"
  [[ "$checkout_container_before" == "$checkout_container_after" ]] || \
    fail "checkout restarted during readiness recovery"
  echo "Checkout readiness failure/recovery passed"
else
  echo "Checkout readiness failure/recovery skipped in external mode"
fi

echo "commerce stack verification passed"

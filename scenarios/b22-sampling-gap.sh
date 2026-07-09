#!/usr/bin/env bash
# B22: sampling gap. Recreate checkout at 10% root sampling, drive 50 requests,
# then restore default 100% sampling.
set -euo pipefail

COMPOSE_FILE="${COMPOSE_FILE:-deploy/docker-compose.yml}"
BASE="${CHECKOUT_BASE:-http://localhost:8088}"

compose() {
  docker compose -f "$COMPOSE_FILE" "$@"
}

wait_checkout() {
  for _ in $(seq 1 30); do
    if curl --max-time 3 -fsS "$BASE/healthz" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "checkout did not become reachable at $BASE" >&2
  return 1
}

restore() {
  unset PLAYGROUND_SAMPLE_RATIO
  compose up -d --no-deps --force-recreate checkout >/dev/null || true
}

trap restore EXIT

export PLAYGROUND_SAMPLE_RATIO=0.1
compose up -d --no-deps --force-recreate checkout >/dev/null
wait_checkout

ok=0
for i in $(seq 1 50); do
  code="$(curl --max-time 10 -sS "$BASE/checkout?sku=WIDGET-1&quantity=$((i % 5 + 1))" -o /dev/null -w "%{http_code}")"
  echo "checkout $i [$code]"
  if [[ "$code" == "200" ]]; then
    ok=$((ok + 1))
  fi
done

echo "B22 drove $ok/50 successful checkout requests with PLAYGROUND_SAMPLE_RATIO=0.1"
echo "Check in Parallax: Traces shows about 5 of 50; Logs still shows all request logs."
echo "The missing traces are the demo: sampled-out evidence, plus dangling log trace links."

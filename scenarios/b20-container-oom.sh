#!/usr/bin/env bash
# B20: recommendation OOM demo. This intentionally kills and restarts a container.
set -euo pipefail

if [[ "${1:-}" != "--yes" ]]; then
  echo "Refusing to run: this scenario intentionally OOM-kills the recommendation container."
  echo "Re-run with: $0 --yes"
  exit 2
fi

COMPOSE="${COMPOSE:-docker compose}"
BASE="${RECOMMENDATION_URL:-http://localhost:8090}"
LEAK_KB="${LEAK_KB:-8192}"
ROUNDS="${ROUNDS:-32}"

$COMPOSE -f deploy/docker-compose.yml -f deploy/docker-compose.limits.yml up -d recommendation

for i in $(seq 1 "$ROUNDS"); do
  echo "leak round $i: ${LEAK_KB}KiB"
  curl -sS "$BASE/recommend?sku=WIDGET-1&leak=$LEAK_KB" -o /dev/null -w "  [%{http_code}]\n" || true
  $COMPOSE -f deploy/docker-compose.yml -f deploy/docker-compose.limits.yml ps recommendation
  sleep 0.5
done

echo "B20 done. Check in Parallax: recommendation telemetry gap/restart window; Docker shows OOM/restart evidence."

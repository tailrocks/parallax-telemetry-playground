#!/usr/bin/env bash
# B6: flip flagd's cacheLeak variant and drive the recommendation leak path.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${RECOMMENDATION_URL:-http://localhost:8091}"
REQUESTS="${B6_REQUESTS:-10}"
SETTLE_SECONDS="${B6_FLAG_SETTLE_SECONDS:-12}"
FLAG_FILE="$ROOT/flags/flagd.json"
BACKUP="$(mktemp)"
COMPOSE=(-f "$ROOT/deploy/docker-compose.yml")

cp "$FLAG_FILE" "$BACKUP"

restore_flag() {
  cp "$BACKUP" "$FLAG_FILE"
  rm -f "$BACKUP"
}

set_cache_leak_flag() {
  local variant="$1"
  python3 - "$FLAG_FILE" "$variant" <<'PY'
import json
import sys

path, variant = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as source:
    data = json.load(source)
data["flags"]["cacheLeak"]["defaultVariant"] = variant
with open(path, "w", encoding="utf-8") as destination:
    json.dump(data, destination, indent=2)
    destination.write("\n")
PY
}

trap restore_flag EXIT

docker compose "${COMPOSE[@]}" up -d flagd recommendation >/dev/null
set_cache_leak_flag on
sleep "$SETTLE_SECONDS"

for i in $(seq 1 "$REQUESTS"); do
  curl --fail-with-body --max-time 20 -sS "$BASE/recommend?sku=LEAK-$i" >/dev/null
  printf 'leak request %s/%s\n' "$i" "$REQUESTS"
done

echo "B6 done — inspect the cacheLeak feature_flag evaluation and recommendation process-memory growth."

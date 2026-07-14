#!/usr/bin/env bash
# B6: drive the recommendation cache-leak path; flagd evaluation is recorded on every request.
set -euo pipefail

BASE="${RECOMMENDATION_URL:-http://localhost:8091}"
REQUESTS="${B6_REQUESTS:-10}"
LEAK_BYTES="${B6_LEAK_BYTES:-1048576}"
for i in $(seq 1 "$REQUESTS"); do
  curl --fail-with-body --max-time 20 -sS "$BASE/recommend?sku=LEAK-$i&leak=$LEAK_BYTES" >/dev/null
  printf 'leak request %s/%s\n' "$i" "$REQUESTS"
done

echo "B6 done — inspect feature_flag evaluation and recommendation process-memory growth."

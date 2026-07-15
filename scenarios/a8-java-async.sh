#!/usr/bin/env bash
# A8: Java fulfillment's Kafka producer/consumer trace-link path.
set -euo pipefail

BASE="${FULFILLMENT_URL:-http://localhost:8093}"
REQUESTS="${A8_REQUESTS:-8}"
for i in $(seq 1 "$REQUESTS"); do
  curl --fail-with-body --max-time 20 -sS -X POST "$BASE/publish?order=a8-$i" >/dev/null
  printf 'published a8-%s\n' "$i"
done

echo "A8 done — inspect Java producer/consumer spans, their link, and the Java-to-Rust notification hop."

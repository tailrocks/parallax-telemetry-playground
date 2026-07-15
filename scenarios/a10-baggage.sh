#!/usr/bin/env bash
# A10: carry tenant.id and user.tier from checkout through its HTTP and gRPC
# descendants using W3C baggage. The stack must already be running.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
TENANT="${A10_TENANT:-tenant-a}"
TIER="${A10_TIER:-pro}"

echo "A10 checkout with tenant=$TENANT tier=$TIER"
curl --fail-with-body --max-time 20 --get "$BASE/checkout" \
  --data-urlencode "tenant=$TENANT" \
  --data-urlencode "tier=$TIER"

cat <<EOF

Check in Parallax: the checkout, inventory, and pricing server spans carry
tenant.id=$TENANT and user.tier=$TIER. The downstream HTTP and gRPC request
carriers include one W3C baggage header with those two values.
EOF

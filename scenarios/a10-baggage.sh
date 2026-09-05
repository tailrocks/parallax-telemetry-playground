#!/usr/bin/env bash
# A10: carry tenant.id and user.tier from checkout through its HTTP and gRPC
# descendants using W3C baggage. The stack must already be running.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
TENANT="${A10_TENANT:-tenant-acme}"
CUSTOMER="${A10_CUSTOMER:-customer-acme-ava}"
TIER="${A10_TIER:-pro}"

echo "A10 checkout with tenant=$TENANT tier=$TIER"
curl --fail-with-body --max-time 20 -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"$TENANT\",\"customer_id\":\"$CUSTOMER\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"tier\":\"$TIER\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"a10-$$\"}"

cat <<EOF

Check in Parallax: the checkout, inventory, and pricing server spans carry
tenant.id=$TENANT and user.tier=$TIER. The downstream HTTP and gRPC request
carriers include W3C baggage with those values.
EOF

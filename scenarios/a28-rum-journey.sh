#!/usr/bin/env bash
set -euo pipefail

WEB_URL="${WEB_URL:-http://localhost:5173}"

echo "A28 frontend RUM journey"
echo
echo "Smoke-check SSR pages:"
for path in / /catalog /cart /checkout /orders /analytics; do
  curl -fsS "$WEB_URL$path" -o /dev/null
  printf "  ok %s\n" "$WEB_URL$path"
done

cat <<STEPS

Manual browser journey:
  1. Open $WEB_URL/
  2. Browse $WEB_URL/catalog and open a seeded WIDGET-1 product.
  3. Add it to cart, open $WEB_URL/cart, and continue to checkout.
  4. Optionally enter promo code ACME10, submit with a normal payment method, and review the order.
  5. Open $WEB_URL/orders and an order detail page, then open $WEB_URL/analytics.
  6. Refresh order status or analytics, then background or close the tab to trigger OTel flush hooks.

Expected Parallax evidence:
  - service.name=web spans carry resource_attributes.session.id.
  - app.screen.name route spans appear for home, catalog, cart, checkout, orders, and analytics.
  - ui.click/ui.submit spans carry app.widget.name.
  - browser.web_vital spans carry web_vital.name/value/rating.
  - Checkout is stitched browser -> storefront -> checkout.
  - web.checkout.submitted and ClickHouse-backed analytics evidence are present.
STEPS

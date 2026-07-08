#!/usr/bin/env bash
set -euo pipefail

WEB_URL="${WEB_URL:-http://localhost:5173}"

echo "A28 frontend RUM journey"
echo
echo "Smoke-check SSR pages:"
for path in / /checkout /orders; do
  curl -fsS "$WEB_URL$path" -o /dev/null
  printf "  ok %s\n" "$WEB_URL$path"
done

cat <<STEPS

Manual browser journey:
  1. Open $WEB_URL/
  2. Click "checkout journey"; submit checkout.
  3. Open "orders"; submit an order.
  4. Return home; click "apply promo (unresponsive)" several times.
  5. Click "break (RUM error)".
  6. Open $WEB_URL/checkout?nopropagate=1 and submit checkout again.
  7. Background or close the tab to trigger the OTel forceFlush hooks.

Expected Parallax evidence:
  - service.name=web spans carry resource_attributes.session.id.
  - app.screen.name route spans appear for home, checkout, and orders.
  - ui.click/ui.submit spans carry app.widget.name.
  - browser.web_vital spans carry web_vital.name/value/rating.
  - Normal checkout is stitched browser -> checkout.
  - nopropagate checkout produces browser and backend traces that do not share a trace id.
STEPS

// k6 load: drives the checkout flow so traces/metrics fan out to every backend.
//   k6 run loadgen/checkout.js
import http from "k6/http";
import { sleep } from "k6";
export const options = { vus: 5, duration: "1m" };
const BASE = __ENV.CHECKOUT_URL || "http://localhost:8088";
export default function () {
  http.get(`${BASE}/checkout?sku=WIDGET-1&quantity=${1 + Math.floor(Math.random() * 5)}`);
  sleep(1);
}

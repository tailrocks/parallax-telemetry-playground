// Long-running ambient demo traffic. Run by the compose `demo` profile.
import http from "k6/http";
import { sleep } from "k6";
import type { Options } from "k6/options";

export const options: Options = { vus: 2, duration: "24h" };

const CHECKOUT_URL = __ENV["CHECKOUT_URL"] ?? "http://checkout:8088";
const ORDERS_URL = __ENV["ORDERS_URL"] ?? "http://orders:8092";
const FULFILLMENT_URL = __ENV["FULFILLMENT_URL"] ?? "http://fulfillment:8080";
const SKUS = ["WIDGET-1", "WIDGET-2", "WIDGET-3", "WIDGET-4"] as const;

function pick<T>(items: readonly [T, ...T[]]): T {
  return items[Math.floor(Math.random() * items.length)] ?? items[0];
}

function randInt(min: number, max: number): number {
  return min + Math.floor(Math.random() * (max - min + 1));
}

export default function demoLoad(): void {
  const sku = pick(SKUS);
  const quantity = randInt(1, 5);
  const roll = Math.random();

  if (roll < 0.8) {
    http.get(`${CHECKOUT_URL}/checkout?sku=${sku}&quantity=${quantity}`);
  } else if (roll < 0.9) {
    http.get(
      `${CHECKOUT_URL}/checkout?sku=${sku}&quantity=${quantity}&slow=${randInt(250, 1500)}`,
    );
  } else if (roll < 0.95) {
    http.get(`${CHECKOUT_URL}/checkout?sku=${sku}&quantity=${quantity}&fail=1`);
  } else {
    http.get(`${CHECKOUT_URL}/quote-stream?sku=${sku}&quantity=4`);
  }

  if (__ITER % 10 === 0) {
    http.post(`${ORDERS_URL}/order`);
    http.post(`${FULFILLMENT_URL}/publish?order=demo-${__VU}-${__ITER}`);
  }

  sleep(randInt(1, 3));
}

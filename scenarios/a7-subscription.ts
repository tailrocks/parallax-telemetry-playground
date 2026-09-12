#!/usr/bin/env bun
// A7: minimal GraphQL-over-WebSocket subscription smoke using Bun's native
// WebSocket and the graphql-transport-ws protocol.

const url = process.env.CATALOG_WS_URL ?? "ws://localhost:8080/graphql";
const httpUrl = process.env.CATALOG_HTTP_URL ?? "http://localhost:8080/graphql";
const catalogAdminToken =
  process.env.CATALOG_ADMIN_TOKEN ||
  "catalog-admin:tenant-acme:research-secret";
const authorization = `Bearer ${catalogAdminToken}`;
const timeoutMs = Number(process.env.SUBSCRIPTION_TIMEOUT_MS ?? "10000");
const id = "price-changes";

let events = 0;
let acknowledged = false;
let restored = false;
let restoreStarted = false;
let finished = false;
let commitTimer: ReturnType<typeof setTimeout> | undefined;

const socket = new WebSocket(url, {
  headers: { Authorization: authorization },
  protocols: ["graphql-transport-ws"],
});

function closeSocket(code: number, reason: string): void {
  if (
    socket.readyState === WebSocket.OPEN ||
    socket.readyState === WebSocket.CONNECTING
  ) {
    socket.close(code, reason);
  }
}

function fail(message: string): void {
  if (finished) return;
  finished = true;
  clearTimeout(timer);
  if (commitTimer) clearTimeout(commitTimer);
  console.error(message);
  closeSocket(1011, "A7 failure");
  process.exit(1);
}

function succeed(): void {
  if (finished) return;
  finished = true;
  clearTimeout(timer);
  if (commitTimer) clearTimeout(commitTimer);
  console.log(`A7 done: received ${events} subscription event(s) from ${url}`);
  closeSocket(1000, "A7 restored");
  process.exit(0);
}

const timer = setTimeout(() => {
  if (events < 1) {
    fail(`A7 failed: no subscription events received from ${url}`);
    return;
  }
  if (!restored) {
    fail(
      `A7 failed: received an event but could not restore the seeded price before timeout`,
    );
    return;
  }
  succeed();
}, timeoutMs);

async function updatePrice(
  amountMinor: number,
  compareAtMinor: number,
  description: string,
): Promise<void> {
  const response = await fetch(httpUrl, {
    method: "POST",
    headers: {
      Authorization: authorization,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      operationName: "CommitPriceChange",
      query: `mutation CommitPriceChange {
        updatePrice(input: {
          tenantId: "tenant-acme",
          sku: "WIDGET-1",
          currency: "USD",
          amountMinor: ${amountMinor},
          compareAtMinor: ${compareAtMinor}
        }) { sku price { amountMinor currency } }
      }`,
    }),
  });
  if (!response.ok) {
    throw new Error(`price update returned HTTP ${response.status}`);
  }
  const payload = (await response.json()) as {
    errors?: readonly { message: string }[];
  };
  if (payload.errors?.length) {
    throw new Error(payload.errors.map((error) => error.message).join("; "));
  }
  console.log(`${description}: WIDGET-1 ${amountMinor} minor units`);
}

async function commitPriceChange(): Promise<void> {
  await updatePrice(2199, 2499, "committed A7 price change");
}

async function restoreSeedPrice(): Promise<void> {
  await updatePrice(1999, 2299, "restored seeded price");
}

socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ type: "connection_init", payload: {} }));
});

socket.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data)) as {
    type: string;
    id?: string;
    payload?: unknown;
  };

  if (message.type === "connection_ack") {
    acknowledged = true;
    socket.send(
      JSON.stringify({
        id,
        type: "subscribe",
        payload: {
          query: `subscription priceSmoke {
            priceChanges(tenantId: "tenant-acme", sku: "WIDGET-1") {
              product { sku name priceMinor }
              variant { sku name }
              price { amountMinor currency }
              observedAt
            }
          }`,
        },
      }),
    );
    commitTimer = setTimeout(() => {
      if (finished) return;
      void commitPriceChange().catch((error: unknown) => {
        fail(
          `A7 failed: could not commit price change: ${error instanceof Error ? error.message : String(error)}`,
        );
      });
    }, 500);
    return;
  }

  if (message.type === "next" && message.id === id) {
    events += 1;
    console.log(JSON.stringify(message.payload));
    if (!restoreStarted) {
      restoreStarted = true;
      void restoreSeedPrice()
        .then(() => {
          restored = true;
          succeed();
        })
        .catch((error: unknown) => {
          fail(
            `A7 failed: could not restore seeded price: ${error instanceof Error ? error.message : String(error)}`,
          );
        });
    }
    return;
  }

  if (message.type === "error") {
    fail(`A7 failed: subscription error ${JSON.stringify(message.payload)}`);
  }
});

socket.addEventListener("close", () => {
  if (!finished) {
    const phase = acknowledged ? "after acknowledgement" : "before acknowledgement";
    fail(
      `A7 failed: websocket closed ${phase}, before event delivery and price restoration from ${url}`,
    );
  }
});

socket.addEventListener("error", () => {
  fail(`A7 failed: websocket error from ${url}`);
});

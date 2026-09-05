// k6 load: drives weighted Storefront journeys through the real commerce API.
//
// Required environment:
//   STOREFRONT_URL=http://localhost:8094/graphql
// Optional environment:
//   LOADGEN_RUN_ID=b16-20260905T120000Z
//   LOADGEN_MODE=mixed|browse|product|cart|checkout|order|analytics
//   LOADGEN_WEIGHTS=browse=30,product=20,cart=15,checkout=20,order=10,analytics=5
//
// Run directly with:
//   STOREFRONT_URL=http://localhost:8094/graphql LOADGEN_RUN_ID=local k6 run loadgen/checkout.ts
import http from "k6/http";
import { check, sleep } from "k6";
import type { Response } from "k6/http";
import type { Options } from "k6/options";

export const options: Options = {
  vus: 5,
  duration: "1m",
  thresholds: { checks: ["rate==1"] },
};

type Workflow =
  | "browse"
  | "product"
  | "cart"
  | "checkout"
  | "order"
  | "analytics";
type Mode = "mixed" | Workflow;
type JsonRecord = Record<string, unknown>;
type BrowseVariant = Readonly<{
  search: string | null;
  category: string | null;
  sort: "FEATURED" | "NEWEST" | "PRICE_ASC" | "PRICE_DESC" | "RELEVANCE";
}>;
type TenantFixture = Readonly<{
  tenantId: string;
  customerId: string;
  promotionCode: string;
  skus: readonly [string, ...string[]];
  browseVariants: readonly [BrowseVariant, ...BrowseVariant[]];
}>;
type RequestScope = Readonly<{
  tenant: TenantFixture;
  customerId: string;
  sku: string;
  sessionId: string;
  cartId: string;
  prefix: string;
}>;
type WorkflowWeights = Record<Workflow, number>;
type WeightedChoice<T> = Readonly<{ item: T; weight: number }>;

const WORKFLOWS = [
  "browse",
  "product",
  "cart",
  "checkout",
  "order",
  "analytics",
] as const satisfies readonly [Workflow, ...Workflow[]];

const DEFAULT_WORKFLOW_WEIGHTS: WorkflowWeights = {
  browse: 30,
  product: 20,
  cart: 15,
  checkout: 20,
  order: 10,
  analytics: 5,
};

const STOREFRONT_GRAPHQL_URL = requiredEnv("STOREFRONT_URL");
const STOREFRONT_HTTP_URL = storefrontHttpUrl(STOREFRONT_GRAPHQL_URL);
const RUN_ID = parseRunId(__ENV["LOADGEN_RUN_ID"]);
const MODE = parseMode(__ENV["LOADGEN_MODE"]);
const WORKFLOW_WEIGHTS = parseWeights(__ENV["LOADGEN_WEIGHTS"]);

const TENANTS = [
  {
    tenantId: "tenant-acme",
    customerId: "customer-acme-ava",
    promotionCode: "ACME10",
    skus: ["WIDGET-1", "WIDGET-2", "GADGET-1", "GADGET-2"],
    browseVariants: [
      { search: null, category: null, sort: "FEATURED" },
      { search: "widget", category: "kitchen", sort: "RELEVANCE" },
      { search: "gadget", category: "electronics", sort: "PRICE_ASC" },
    ],
  },
  {
    tenantId: "tenant-nova",
    customerId: "customer-nova-mia",
    promotionCode: "NOVA15",
    skus: [
      "NOVA-PACK-20",
      "NOVA-PACK-30",
      "NOVA-LAMP-DESK",
      "NOVA-LAMP-FLOOR",
    ],
    browseVariants: [
      { search: null, category: null, sort: "FEATURED" },
      { search: "pack", category: "packs", sort: "RELEVANCE" },
      { search: "lamp", category: "lighting", sort: "PRICE_DESC" },
    ],
  },
] as const satisfies readonly [TenantFixture, TenantFixture];

const TENANT_CHOICES = [
  { item: TENANTS[0], weight: 70 },
  { item: TENANTS[1], weight: 30 },
] as const;

const BROWSE_QUERY = `
  query LoadgenBrowse(
    $search: String,
    $category: String,
    $sort: ProductSort,
    $tenantId: String!,
    $page: Int!,
    $size: Int!,
    $segment: String!
  ) {
    products(
      search: $search,
      category: $category,
      sort: $sort,
      tenantId: $tenantId,
      page: $page,
      size: $size,
      segment: $segment
    ) {
      items { sku tenantId name priceMinor }
      page size totalElements totalPages hasNext experience
    }
    categories(tenantId: $tenantId) { slug name }
  }
`;

const PRODUCT_QUERY = `
  query LoadgenProduct($sku: String!, $tenantId: String!, $segment: String!) {
    product(sku: $sku, tenantId: $tenantId, segment: $segment) {
      sku tenantId name priceMinor
      variants { sku name price { currency amountMinor } }
      reviews { stars title }
    }
  }
`;

const ADD_CART_ITEM_MUTATION = `
  mutation LoadgenAddCartItem($input: AddCartItemInput!) {
    addCartItem(input: $input) {
      cartId sku quantityAdded unitPriceMinor
    }
  }
`;

const CART_QUERY = `
  query LoadgenCart(
    $tenantId: String!,
    $customerId: String!,
    $sessionId: String!
  ) {
    cart(
      tenantId: $tenantId,
      customerId: $customerId,
      sessionId: $sessionId
    ) {
      id status currency
      items { sku productName quantity unitPriceMinor lineTotalMinor }
    }
  }
`;

const CHECKOUT_MUTATION = `
  mutation LoadgenCheckout($input: CheckoutInput!) {
    checkout(input: $input) {
      orderId status paymentStatus currency totalMinor eventKey featureVariant
    }
  }
`;

const ANALYTICS_MUTATION = `
  mutation LoadgenRecordAnalytics($input: AnalyticsInput!) {
    recordAnalytics(input: $input) { eventKey status }
  }
`;

const ANALYTICS_QUERY = `
  query LoadgenAnalytics(
    $tenantId: String!,
    $eventName: String!,
    $limit: Int!
  ) {
    analyticsEvents(tenantId: $tenantId, eventName: $eventName, limit: $limit) {
      eventId tenantId eventKey customerId eventName entityType entityId
      occurredAt
    }
    analyticsSummary(tenantId: $tenantId, eventName: $eventName) {
      tenantId eventName eventCount uniqueCustomers firstOccurredAt lastOccurredAt
    }
  }
`;

const JSON_HEADERS = { "Content-Type": "application/json" };

export function runStorefrontIteration(): number {
  const random = seededRandom(`${RUN_ID}:${__VU}:${__ITER}`);
  const workflow = chooseWorkflow(random);
  const scope = createScope(random);

  switch (workflow) {
    case "browse":
      runBrowse(scope, random);
      break;
    case "product":
      runProduct(scope);
      break;
    case "cart":
      runCart(scope, random);
      break;
    case "checkout":
      runCheckout(scope, random);
      break;
    case "order":
      runOrder(scope, random);
      break;
    case "analytics":
      runAnalytics(scope);
      break;
  }

  return randomInt(random, 1, 3);
}

export default function checkoutLoad(): void {
  sleep(runStorefrontIteration());
}

function runBrowse(scope: RequestScope, random: () => number): void {
  const variant = choose(scope.tenant.browseVariants, random);
  const data = graphql("LoadgenBrowse", BROWSE_QUERY, {
    search: variant.search,
    category: variant.category,
    sort: variant.sort,
    tenantId: scope.tenant.tenantId,
    page: 0,
    size: 20,
    segment: "standard",
  });
  const products = requireRecord(data["products"], "browse products");
  const productsItems = requireArray(products["items"], "browse products.items");
  if (productsItems.length === 0) {
    throw new Error("Storefront browse returned no seeded products");
  }
  const firstProduct = requireRecord(productsItems[0], "browse product");
  requireString(firstProduct["sku"], "browse product sku");
  if (firstProduct["tenantId"] !== scope.tenant.tenantId) {
    throw new Error("Storefront browse returned a product for the wrong tenant");
  }
  const categories = requireArray(data["categories"], "browse categories");
  if (categories.length === 0) {
    throw new Error("Storefront browse returned no seeded categories");
  }
}

function runProduct(scope: RequestScope): void {
  const data = graphql("LoadgenProduct", PRODUCT_QUERY, {
    sku: scope.sku,
    tenantId: scope.tenant.tenantId,
    segment: "standard",
  });
  const product = requireRecord(data["product"], "product");
  requireExpectedString(product["sku"], scope.sku, "product sku");
  requireExpectedString(product["tenantId"], scope.tenant.tenantId, "product tenant");
  requireString(product["name"], "product name");
  requireArray(product["variants"], "product variants");
}

function runCart(scope: RequestScope, random: () => number): void {
  const quantity = randomInt(random, 1, 2);
  addCartItem(scope, quantity);
  const data = graphql("LoadgenCart", CART_QUERY, {
    tenantId: scope.tenant.tenantId,
    customerId: scope.customerId,
    sessionId: scope.sessionId,
  });
  const cart = requireRecord(data["cart"], "cart");
  requireExpectedString(cart["id"], scope.cartId, "cart id");
  requireExpectedString(cart["status"], "active", "cart status");
  requireExpectedString(cart["currency"], "USD", "cart currency");
  const items = requireArray(cart["items"], "cart items");
  const item = items
    .map((value) => (isRecord(value) ? value : null))
    .find((value) => value?.["sku"] === scope.sku);
  if (item === undefined || item === null) {
    throw new Error("Storefront cart omitted the added seeded SKU");
  }
  requireNumberAtLeast(item["quantity"], quantity, "cart item quantity");
}

function runCheckout(scope: RequestScope, random: () => number): void {
  const quantity = randomInt(random, 1, 2);
  addCartItem(scope, quantity);
  const paymentMethodType = choose(
    ["card", "bank_account", "wallet"] as const,
    random,
  );
  const data = graphql("LoadgenCheckout", CHECKOUT_MUTATION, {
    input: {
      items: [{ sku: scope.sku, quantity }],
      tenantId: scope.tenant.tenantId,
      customerId: scope.customerId,
      cartId: scope.cartId,
      sessionId: scope.sessionId,
      currencyCode: "USD",
      promotionCode: scope.tenant.promotionCode,
      paymentMethodToken: "tok_visa",
      paymentMethodType,
      segment: "standard",
      requestId: scopedId(scope, "checkout-request"),
    },
  });
  const receipt = requireRecord(data["checkout"], "checkout receipt");
  const orderId = requireString(receipt["orderId"], "checkout order id");
  requireString(receipt["status"], "checkout status");
  requireExpectedString(receipt["currency"], "USD", "checkout currency");
  readOrder(scope, orderId, true);
}

function runOrder(scope: RequestScope, random: () => number): void {
  const data = storefrontGet(
    `/api/orders?tenant_id=${encodeURIComponent(scope.tenant.tenantId)}&customer_id=${encodeURIComponent(scope.customerId)}`,
    "LoadgenOrders",
  );
  const orders = requireArray(data["orders"], "orders");
  if (orders.length === 0) {
    throw new Error("Storefront order list returned no seeded orders");
  }
  const summary = requireRecord(choose(orders, random), "order summary");
  const orderId = requireString(summary["id"], "order summary id");
  readOrder(scope, orderId, false);
}

function runAnalytics(scope: RequestScope): void {
  const eventName = "loadgen.journey";
  const eventKey = scopedId(scope, "analytics-event");
  const data = graphql("LoadgenRecordAnalytics", ANALYTICS_MUTATION, {
    input: {
      tenantId: scope.tenant.tenantId,
      eventKey,
      eventName,
      entityType: "product",
      entityId: scope.sku,
      customerId: scope.customerId,
      sessionId: scope.sessionId,
      occurredAt: new Date().toISOString(),
      properties: JSON.stringify({
        source: "k6",
        runId: RUN_ID,
        workflow: "analytics",
        sku: scope.sku,
      }),
    },
  });
  const acknowledgement = requireRecord(
    data["recordAnalytics"],
    "analytics acknowledgement",
  );
  requireExpectedString(acknowledgement["eventKey"], eventKey, "analytics event key");
  requireExpectedString(acknowledgement["status"], "recorded", "analytics status");

  const readData = graphql("LoadgenAnalytics", ANALYTICS_QUERY, {
    tenantId: scope.tenant.tenantId,
    eventName,
    limit: 20,
  });
  const events = requireArray(readData["analyticsEvents"], "analytics events");
  const eventFound = events.some(
    (value) => isRecord(value) && value["eventKey"] === eventKey,
  );
  if (!eventFound) {
    throw new Error("Storefront analytics read omitted the recorded event");
  }
  const summary = requireRecord(readData["analyticsSummary"], "analytics summary");
  requireNumberAtLeast(summary["eventCount"], 1, "analytics event count");
}

function addCartItem(scope: RequestScope, quantity: number): void {
  const data = graphql("LoadgenAddCartItem", ADD_CART_ITEM_MUTATION, {
    input: {
      sku: scope.sku,
      quantity,
      tenantId: scope.tenant.tenantId,
      customerId: scope.customerId,
      cartId: scope.cartId,
      sessionId: scope.sessionId,
      currencyCode: "USD",
    },
  });
  const added = requireRecord(data["addCartItem"], "cart item acknowledgement");
  requireExpectedString(added["cartId"], scope.cartId, "cart acknowledgement id");
  requireExpectedString(added["sku"], scope.sku, "cart acknowledgement sku");
  requireExpectedNumber(added["quantityAdded"], quantity, "cart acknowledgement quantity");
}

function readOrder(scope: RequestScope, orderId: string, includeSession: boolean): void {
  const sessionQuery = includeSession
    ? `&session_id=${encodeURIComponent(scope.sessionId)}`
    : "";
  const data = storefrontGet(
    `/api/orders/${encodeURIComponent(orderId)}?tenant_id=${encodeURIComponent(scope.tenant.tenantId)}&customer_id=${encodeURIComponent(scope.customerId)}${sessionQuery}`,
    includeSession ? "LoadgenCheckoutOrder" : "LoadgenOrder",
  );
  requireExpectedString(data["id"], orderId, "order id");
  requireExpectedString(data["tenant_id"], scope.tenant.tenantId, "order tenant");
  requireExpectedString(data["customer_id"], scope.customerId, "order customer");
}

function graphql(
  operation: string,
  query: string,
  variables: Readonly<Record<string, unknown>>,
): JsonRecord {
  const response = http.post(
    STOREFRONT_GRAPHQL_URL,
    JSON.stringify({ operationName: operation, query, variables }),
    { headers: JSON_HEADERS },
  );
  requireHttpSuccess(response, operation);
  const payload = responseJson(response, operation);
  const errors = payload["errors"];
  if (Array.isArray(errors) && errors.length > 0) {
    throw new Error(`${operation} returned GraphQL errors`);
  }
  return requireRecord(payload["data"], `${operation} data`);
}

function storefrontGet(path: string, operation: string): JsonRecord {
  const response = http.get(`${STOREFRONT_HTTP_URL}${path}`);
  requireHttpSuccess(response, operation);
  return responseJson(response, operation);
}

function requireHttpSuccess(response: Response, operation: string): void {
  const passed = check(response, {
    [`${operation} succeeds`]: (result: Response) => result.status === 200,
  });
  if (!passed) {
    throw new Error(`${operation} returned HTTP ${response.status}`);
  }
}

function responseJson(response: Response, operation: string): JsonRecord {
  let value: unknown;
  try {
    value = response.json();
  } catch {
    throw new Error(`${operation} returned invalid JSON`);
  }
  return requireRecord(value, `${operation} JSON`);
}

function createScope(random: () => number): RequestScope {
  const tenant = chooseWeighted<TenantFixture>(TENANT_CHOICES, random);
  const prefix = `loadgen-${RUN_ID}-v${__VU}-i${__ITER}`;
  return {
    tenant,
    customerId: tenant.customerId,
    sku: choose(tenant.skus, random),
    sessionId: `${prefix}-session`,
    cartId: `${prefix}-${tenant.tenantId}-cart`,
    prefix,
  };
}

function scopedId(scope: RequestScope, kind: string): string {
  return `${scope.prefix}-${kind}`;
}

function chooseWorkflow(random: () => number): Workflow {
  if (MODE !== "mixed") return MODE;
  const total = totalWeight(WORKFLOW_WEIGHTS);
  let cursor = random() * total;
  for (const workflow of WORKFLOWS) {
    cursor -= WORKFLOW_WEIGHTS[workflow];
    if (cursor < 0) return workflow;
  }
  throw new Error("workflow weights did not select a workflow");
}

function chooseWeighted<T>(
  choices: readonly [WeightedChoice<T>, ...WeightedChoice<T>[]],
  random: () => number,
): T {
  let cursor = random() * choices.reduce((sum, choice) => sum + choice.weight, 0);
  for (const choice of choices) {
    cursor -= choice.weight;
    if (cursor < 0) return choice.item;
  }
  throw new Error("weighted choices did not select an item");
}

function choose<T>(items: readonly T[], random: () => number): T {
  const item = items[Math.floor(random() * items.length)];
  if (item === undefined) throw new Error("seeded choice was empty");
  return item;
}

function randomInt(random: () => number, min: number, max: number): number {
  return min + Math.floor(random() * (max - min + 1));
}

function seededRandom(seed: string): () => number {
  let state = hashSeed(seed) || 0x9e3779b9;
  return () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    state >>>= 0;
    return state / 0x100000000;
  };
}

function hashSeed(value: string): number {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash = Math.imul(hash ^ value.charCodeAt(index), 16777619);
  }
  return hash >>> 0;
}

function requiredEnv(name: string): string {
  const value = __ENV[name]?.trim();
  if (value === undefined || value.length === 0) {
    throw new Error(`${name} must identify the Storefront GraphQL endpoint`);
  }
  return value;
}

function storefrontHttpUrl(graphqlUrl: string): string {
  const match = /^(.*)\/graphql\/?$/.exec(graphqlUrl);
  const base = match?.[1];
  if (base === undefined || base.length === 0) {
    throw new Error("STOREFRONT_URL must end in /graphql");
  }
  return base;
}

function parseRunId(raw: string | undefined): string {
  const value = raw?.trim() || "demo";
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,47}$/.test(value)) {
    throw new Error("LOADGEN_RUN_ID must be 1-48 letters, numbers, dots, _ or -");
  }
  return value;
}

function parseMode(raw: string | undefined): Mode {
  const value = raw?.trim() || "mixed";
  if (value === "mixed" || isWorkflow(value)) return value;
  throw new Error(`LOADGEN_MODE is unsupported: ${value}`);
}

function parseWeights(raw: string | undefined): WorkflowWeights {
  const weights: WorkflowWeights = { ...DEFAULT_WORKFLOW_WEIGHTS };
  const value = raw?.trim();
  if (value === undefined || value.length === 0) return weights;

  for (const entry of value.split(",")) {
    const parts = entry.split("=");
    const name = parts[0]?.trim();
    const weightText = parts[1]?.trim();
    if (
      parts.length !== 2 ||
      name === undefined ||
      weightText === undefined ||
      !isWorkflow(name)
    ) {
      throw new Error(`LOADGEN_WEIGHTS entry is invalid: ${entry}`);
    }
    const weight = Number(weightText);
    if (!Number.isInteger(weight) || weight < 0) {
      throw new Error(`LOADGEN_WEIGHTS value is invalid: ${entry}`);
    }
    weights[name] = weight;
  }
  if (totalWeight(weights) <= 0) {
    throw new Error("LOADGEN_WEIGHTS must include a positive total");
  }
  return weights;
}

function totalWeight(weights: WorkflowWeights): number {
  return WORKFLOWS.reduce((sum, workflow) => sum + weights[workflow], 0);
}

function isWorkflow(value: string): value is Workflow {
  return (
    value === "browse" ||
    value === "product" ||
    value === "cart" ||
    value === "checkout" ||
    value === "order" ||
    value === "analytics"
  );
}

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requireRecord(value: unknown, label: string): JsonRecord {
  if (!isRecord(value)) throw new Error(`${label} is missing or invalid`);
  return value;
}

function requireArray(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${label} is missing or invalid`);
  return value;
}

function requireString(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} is missing or invalid`);
  }
  return value;
}

function requireExpectedString(value: unknown, expected: string, label: string): void {
  if (requireString(value, label) !== expected) {
    throw new Error(`${label} did not match the requested seeded identity`);
  }
}

function requireExpectedNumber(value: unknown, expected: number, label: string): void {
  if (typeof value !== "number" || value !== expected) {
    throw new Error(`${label} did not match the requested quantity`);
  }
}

function requireNumberAtLeast(value: unknown, expected: number, label: string): void {
  if (typeof value !== "number" || value < expected) {
    throw new Error(`${label} is missing or below the requested quantity`);
  }
}

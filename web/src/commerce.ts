import { reportHandledError, tracedFetch } from "./rum";
import { formatGraphqlErrorPaths, type GraphqlPathPart } from "./graphql-errors";
import {
  SAFE_WEB_ERROR_MESSAGES,
  safeWebMessage,
  type SafeWebErrorCode,
} from "./error-contract";

export const DEMO_TENANT_ID = "tenant-acme" as const;
export const DEMO_CUSTOMER_ID = "customer-acme-ava" as const;
export const DEMO_CURRENCY = "USD" as const;
export const DEMO_PAYMENT_TOKEN = "tok_visa" as const;

const STOREFRONT_GRAPHQL_URL = storefrontGraphqlUrl();

function storefrontGraphqlUrl(): string {
  const configuredBrowserUrl = import.meta.env["VITE_STOREFRONT_URL"]?.trim();
  if (typeof window !== "undefined") {
    if (configuredBrowserUrl === undefined || configuredBrowserUrl.length === 0) {
      return "/__storefront/graphql";
    }
    return configuredBrowserUrl.startsWith("/")
      ? new URL(configuredBrowserUrl, window.location.origin).toString()
      : configuredBrowserUrl;
  }
  return (
    process.env["STOREFRONT_URL"]?.trim() || "http://localhost:8094/graphql"
  );
}

export type Category = Readonly<{
  id: string;
  tenantId: string;
  slug: string;
  name: string;
}>;

export type PriceSnapshot = Readonly<{
  id: string;
  currency: string;
  amountMinor: number;
  compareAtMinor: number | null;
  validFrom: string;
}>;

export type ProductVariant = Readonly<{
  id: string;
  tenantId: string;
  productId: string;
  sku: string;
  name: string;
  options: string;
  price: PriceSnapshot | null;
}>;

export type Review = Readonly<{
  id: string;
  productId: string;
  title: string;
  text: string;
  stars: number;
  verifiedPurchase: boolean;
  createdAt: string;
}>;

export type Product = Readonly<{
  id: string;
  tenantId: string;
  slug: string;
  sku: string;
  name: string;
  description: string;
  brand: string | null;
  category: Category;
  priceMinor: number | null;
  price: PriceSnapshot | null;
  variants: readonly ProductVariant[];
  reviews: readonly Review[];
  reviewsSlow: readonly Review[];
  riskScore: number | null;
}>;

export type ProductPage = Readonly<{
  items: readonly Product[];
  page: number;
  size: number;
  totalElements: number;
  totalPages: number;
  hasNext: boolean;
  experience: string;
}>;

export type CatalogSort =
  | "FEATURED"
  | "NEWEST"
  | "PRICE_ASC"
  | "PRICE_DESC"
  | "RELEVANCE";

export type Money = Readonly<{
  currencyCode: string;
  amountMinor: number;
}>;

export type QuoteLine = Readonly<{
  sku: string;
  quantity: number;
  unitPrice: Money | null;
  lineTotal: Money | null;
}>;

export type Quote = Readonly<{
  quoteId: string;
  status: string;
  lines: readonly QuoteLine[];
  subtotal: Money | null;
  discountTotal: Money | null;
  taxTotal: Money | null;
  grandTotal: Money | null;
  validForSeconds: number;
  pricingVersion: string;
}>;

export type CartEntry = Readonly<{
  sku: string;
  quantity: number;
}>;

export type CheckoutReceipt = Readonly<{
  status: string;
  orderId: string | null;
  orderNumber: string | null;
  tenantId: string | null;
  customerId: string | null;
  currency: string | null;
  subtotalMinor: number | null;
  discountMinor: number | null;
  totalMinor: number | null;
  paymentId: string | null;
  paymentStatus: string | null;
  featureVariant: string | null;
}>;

export type OrderSummary = Readonly<{
  id: string;
  orderNumber: string;
  status: string;
  currency: string;
  totalMinor: number;
  createdAt: string | null;
}>;

export type OrderItem = Readonly<{
  sku: string;
  productName: string;
  quantity: number;
  unitPriceMinor: number;
  discountMinor: number;
  lineTotalMinor: number;
}>;

export type OrderDetail = Readonly<
  OrderSummary & {
    customerId: string | null;
    paymentStatus: string | null;
    subtotalMinor: number;
    discountMinor: number;
    taxMinor: number;
    shippingMinor: number;
    items: readonly OrderItem[];
  }
>;

export type AnalyticsEvent = Readonly<{
  eventId: string;
  tenantId: string;
  eventKey: string;
  customerId: string | null;
  eventName: string;
  eventVersion: number;
  source: string;
  entityType: string;
  entityId: string;
  occurredAt: string;
  traceId: string;
  spanId: string;
  properties: string;
}>;

export type AnalyticsSummary = Readonly<{
  tenantId: string;
  eventName: string | null;
  eventCount: number;
  uniqueCustomers: number;
  firstOccurredAt: string | null;
  lastOccurredAt: string | null;
}>;

export class CommerceApiError extends Error {
  readonly kind: "network" | "http" | "graphql" | "decode";
  readonly code: SafeWebErrorCode;
  readonly operation: string;
  readonly status: number | undefined;
  readonly graphqlErrors: readonly GraphqlError[];

  constructor(
    kind: CommerceApiError["kind"],
    operation: string,
    code: SafeWebErrorCode,
    status?: number,
    graphqlErrors: readonly GraphqlError[] = [],
  ) {
    super(SAFE_WEB_ERROR_MESSAGES[code]);
    this.name = "CommerceApiError";
    this.kind = kind;
    this.code = code;
    this.operation = operation;
    this.status = status;
    this.graphqlErrors = graphqlErrors;
  }
}

export type GraphqlError = Readonly<{
  code: "graphql_field_error";
  path?: readonly GraphqlPathPart[];
}>;

type GraphqlEnvelope = Readonly<{
  data: unknown;
  errors: readonly GraphqlError[];
}>;

const PRODUCTS_QUERY = `
  query StorefrontProducts(
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
      items {
        id tenantId slug sku name description brand priceMinor
        category { id tenantId slug name }
        price { id currency amountMinor compareAtMinor validFrom }
        variants {
          id tenantId productId sku name options
          price { id currency amountMinor compareAtMinor validFrom }
        }
        reviews { id productId title text stars verifiedPurchase createdAt }
        riskScore
      }
      page size totalElements totalPages hasNext experience
    }
  }
`;

const PRODUCT_QUERY = `
  query StorefrontProduct($sku: String!, $tenantId: String!, $segment: String!) {
    product(sku: $sku, tenantId: $tenantId, segment: $segment) {
      id tenantId slug sku name description brand priceMinor
      category { id tenantId slug name }
      price { id currency amountMinor compareAtMinor validFrom }
      variants {
        id tenantId productId sku name options
        price { id currency amountMinor compareAtMinor validFrom }
      }
      reviews { id productId title text stars verifiedPurchase createdAt }
      riskScore
    }
  }
`;

const CATEGORIES_QUERY = `
  query StorefrontCategories($tenantId: String!) {
    categories(tenantId: $tenantId) { id tenantId slug name }
  }
`;

const QUOTE_QUERY = `
  query StorefrontQuote($input: QuoteInput!) {
    quote(input: $input) {
      quoteId status
      lines {
        sku quantity
        unitPrice { currencyCode amountMinor }
        lineTotal { currencyCode amountMinor }
      }
      subtotal { currencyCode amountMinor }
      discountTotal { currencyCode amountMinor }
      taxTotal { currencyCode amountMinor }
      grandTotal { currencyCode amountMinor }
      validForSeconds pricingVersion
    }
  }
`;

const CHECKOUT_MUTATION = `
  mutation StorefrontCheckout($input: CheckoutInput!) {
    checkout(input: $input) {
      orderId
      status
      paymentStatus
      currency
      totalMinor
      eventKey
      featureVariant
    }
  }
`;

const ANALYTICS_QUERY = `
  query StorefrontAnalytics(
    $tenantId: String!,
    $eventName: String,
    $limit: Int
  ) {
    analyticsEvents(tenantId: $tenantId, eventName: $eventName, limit: $limit) {
      eventId tenantId eventKey customerId eventName eventVersion source
      entityType entityId occurredAt traceId spanId properties
    }
    analyticsSummary(tenantId: $tenantId, eventName: $eventName) {
      tenantId eventName eventCount uniqueCustomers firstOccurredAt lastOccurredAt
    }
  }
`;

const RECORD_ANALYTICS_MUTATION = `
  mutation RecordStorefrontAnalytics($input: AnalyticsInput!) {
    recordAnalytics(input: $input) { eventKey status }
  }
`;

export async function fetchProducts(
  input: Readonly<{
    search?: string | undefined;
    category?: string | undefined;
    sort?: CatalogSort;
    tenantId: string;
    page?: number;
    size?: number;
    segment?: string;
    signal?: AbortSignal;
  }>,
): Promise<ProductPage> {
  const data = await requestGraphql(
    "StorefrontProducts",
    PRODUCTS_QUERY,
    {
      search: input.search?.trim() || null,
      category: input.category?.trim() || null,
      sort: input.sort ?? "FEATURED",
      tenantId: input.tenantId,
      page: input.page ?? 0,
      size: input.size ?? 20,
      segment: input.segment ?? "standard",
    },
    input.signal,
  );
  const record = getRequiredRecord(data, "products response");
  return parseProductPage(getRequiredRecord(record["products"], "products"));
}

export async function fetchProduct(
  sku: string,
  tenantId: string,
  segment = "standard",
): Promise<Product | null> {
  const data = await requestGraphql("StorefrontProduct", PRODUCT_QUERY, {
    sku,
    tenantId,
    segment,
  });
  const record = getRequiredRecord(data, "product response");
  const value = record["product"];
  return value === null ? null : parseProduct(value);
}

export async function fetchCategories(
  tenantId: string,
  signal?: AbortSignal,
): Promise<readonly Category[]> {
  const data = await requestGraphql(
    "StorefrontCategories",
    CATEGORIES_QUERY,
    { tenantId },
    signal,
  );
  const record = getRequiredRecord(data, "categories response");
  return getRequiredArray(record["categories"], "categories").map(
    parseCategory,
  );
}

export async function fetchQuote(
  input: Readonly<{
    items: readonly CartEntry[];
    customerId: string;
    currencyCode: string;
    promotionCode?: string;
    pricingStrategy?: string;
    paymentMethodType?: string;
    requestId?: string;
  }>,
): Promise<Quote> {
  const data = await requestGraphql("StorefrontQuote", QUOTE_QUERY, {
    input: {
      items: input.items.map((item) => ({
        sku: item.sku,
        quantity: item.quantity,
      })),
      customerId: input.customerId,
      currencyCode: input.currencyCode,
      promotionCode: input.promotionCode || null,
      pricingStrategy: input.pricingStrategy || null,
      paymentMethodType: input.paymentMethodType ?? "card",
      requestId: input.requestId ?? `web-quote-${crypto.randomUUID()}`,
    },
  });
  return parseQuote(
    getRequiredRecord(
      getRequiredRecord(data, "quote response")["quote"],
      "quote",
    ),
  );
}

export async function submitCheckout(
  input: Readonly<{
    items: readonly CartEntry[];
    cartId?: string | null;
    tenantId: string;
    customerId: string;
    currencyCode: string;
    promotionCode?: string;
    paymentMethodToken: string;
    paymentMethodType?: string;
    segment?: string;
    requestId?: string;
  }>,
): Promise<CheckoutReceipt> {
  const data = await requestGraphql("StorefrontCheckout", CHECKOUT_MUTATION, {
    input: {
      items: input.items.map((item) => ({
        sku: item.sku,
        quantity: item.quantity,
      })),
      tenantId: input.tenantId,
      customerId: input.customerId,
      cartId: input.cartId ?? null,
      currencyCode: input.currencyCode,
      promotionCode: input.promotionCode || null,
      paymentMethodToken: input.paymentMethodToken,
      paymentMethodType: input.paymentMethodType ?? "card",
      segment: input.segment ?? "standard",
      requestId: input.requestId ?? `web-checkout-${crypto.randomUUID()}`,
    },
  });
  const record = getRequiredRecord(data, "checkout response");
  return parseCheckoutReceipt(
    getRequiredRecord(record["checkout"], "checkout result"),
  );
}

export async function fetchOrders(
  input: Readonly<{
    tenantId: string;
    customerId: string;
  }>,
): Promise<readonly OrderSummary[]> {
  const params = new URLSearchParams({
    tenant_id: input.tenantId,
  });
  params.set("customer_id", input.customerId);
  const value = await requestJson(
    "StorefrontOrders",
    `/api/orders?${params.toString()}`,
    undefined,
    "GET",
  );
  const record = getRequiredRecord(value, "orders response");
  return getRequiredArray(record["orders"], "orders").map(parseOrderSummary);
}

export async function fetchOrder(
  orderId: string,
  tenantId: string,
  customerId: string,
): Promise<OrderDetail> {
  const params = new URLSearchParams({
    tenant_id: tenantId,
    customer_id: customerId,
  });
  const value = await requestJson(
    "StorefrontOrder",
    `/api/orders/${encodeURIComponent(orderId)}?${params.toString()}`,
    undefined,
    "GET",
  );
  return parseOrderDetail(value);
}

export async function fetchAnalytics(
  input: Readonly<{
    tenantId: string;
    eventName?: string | undefined;
    limit?: number;
  }>,
): Promise<
  Readonly<{ events: readonly AnalyticsEvent[]; summary: AnalyticsSummary }>
> {
  const data = await requestGraphql("StorefrontAnalytics", ANALYTICS_QUERY, {
    tenantId: input.tenantId,
    eventName: input.eventName || null,
    limit: input.limit ?? 50,
  });
  const record = getRequiredRecord(data, "analytics response");
  return {
    events: getRequiredArray(record["analyticsEvents"], "analytics events").map(
      parseAnalyticsEvent,
    ),
    summary: parseAnalyticsSummary(
      getRequiredRecord(record["analyticsSummary"], "analytics summary"),
    ),
  };
}

export async function recordAnalytics(
  input: Readonly<{
    eventKey: string;
    eventName: string;
    entityType: string;
    entityId: string;
    customerId: string;
    sessionId?: string;
    occurredAt?: string;
    properties?: Readonly<Record<string, string | number | boolean>>;
    tenantId: string;
  }>,
): Promise<Readonly<{ eventKey: string; status: string }>> {
  const data = await requestGraphql(
    "RecordStorefrontAnalytics",
    RECORD_ANALYTICS_MUTATION,
    {
      input: {
        tenantId: input.tenantId,
        eventKey: input.eventKey,
        eventName: input.eventName,
        entityType: input.entityType,
        entityId: input.entityId,
        customerId: input.customerId,
        sessionId: input.sessionId,
        occurredAt: input.occurredAt ?? new Date().toISOString(),
        properties: JSON.stringify(input.properties ?? {}),
      },
    },
  );
  const receipt = getRequiredRecord(
    getRequiredRecord(data, "analytics mutation response")["recordAnalytics"],
    "analytics acknowledgement",
  );
  return {
    eventKey: getRequiredString(receipt["eventKey"], "eventKey"),
    status: getRequiredString(receipt["status"], "status"),
  };
}

async function requestGraphql(
  operation: string,
  query: string,
  variables: Readonly<Record<string, unknown>>,
  signal?: AbortSignal,
): Promise<unknown> {
  const value = await requestJson(
    operation,
    STOREFRONT_GRAPHQL_URL,
    { operationName: operation, query, variables },
    "POST",
    signal,
  );
  let envelope: GraphqlEnvelope;
  try {
    envelope = parseGraphqlEnvelope(value, operation);
  } catch (error: unknown) {
    reportHandledError(error, operation, { error_kind: "graphql_decode" });
    throw error;
  }
  const errors = envelope.errors ?? [];
  if (envelope.data === undefined || envelope.data === null) {
    const failure = new CommerceApiError(
      "graphql",
      operation,
      "graphql_failure",
      undefined,
      errors,
    );
    reportHandledError(failure, operation, { error_kind: "graphql" });
    throw failure;
  }
  if (errors.length > 0) {
    const degraded = new CommerceApiError(
      "graphql",
      operation,
      "graphql_failure",
      undefined,
      errors,
    );
    reportHandledError(degraded, operation, {
      error_kind: "graphql",
      graphql_error_count: errors.length,
      graphql_error_paths: formatGraphqlErrorPaths(errors, 128),
    });
  }
  return envelope.data;
}

async function requestJson(
  operation: string,
  path: string,
  body?: unknown,
  method: "GET" | "POST" = "POST",
  signal?: AbortSignal,
): Promise<unknown> {
  const url = path.startsWith("http")
    ? path
    : new URL(path, STOREFRONT_GRAPHQL_URL).toString();
  let response: Response;
  try {
    response = await tracedFetch(url, {
      method,
      ...(signal === undefined ? {} : { signal }),
      ...(body === undefined
        ? {}
        : {
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
      }),
    });
  } catch (error: unknown) {
    const failure = new CommerceApiError(
      "network",
      operation,
      "network_unavailable",
    );
    reportHandledError(failure, operation, { error_kind: "network" });
    throw failure;
  }

  let text: string;
  try {
    text = await response.text();
  } catch (error: unknown) {
    const failure = new CommerceApiError(
      "decode",
      operation,
      "response_invalid",
    );
    reportHandledError(failure, operation, { error_kind: "response_decode" });
    throw failure;
  }
  let value: unknown;
  try {
    value = text.length === 0 ? {} : (JSON.parse(text) as unknown);
  } catch (error: unknown) {
    const failure = new CommerceApiError(
      "decode",
      operation,
      "response_invalid",
    );
    reportHandledError(failure, operation, { error_kind: "json_decode" });
    throw failure;
  }
  if (!response.ok) {
    const failure = new CommerceApiError(
      "http",
      operation,
      "http_failure",
      response.status,
    );
    reportHandledError(failure, operation, {
      error_kind: "http",
      http_status: response.status,
    });
    throw failure;
  }
  return value;
}

function parseProductPage(value: Record<string, unknown>): ProductPage {
  return {
    items: getRequiredArray(value["items"], "product page items").map(
      parseProduct,
    ),
    page: getRequiredInt(value["page"], "product page number"),
    size: getRequiredInt(value["size"], "product page size"),
    totalElements: getRequiredInt(
      value["totalElements"],
      "product page total elements",
    ),
    totalPages: getRequiredInt(value["totalPages"], "product page total pages"),
    hasNext: getRequiredBoolean(value["hasNext"], "product page next flag"),
    experience: getRequiredString(
      value["experience"],
      "product page experience",
    ),
  };
}

function parseGraphqlEnvelope(
  value: unknown,
  operation: string,
): GraphqlEnvelope {
  const record = getRequiredRecord(value, `${operation} GraphQL envelope`);
  const errors = record["errors"];
  if (errors === undefined) return { data: record["data"], errors: [] };
  if (!Array.isArray(errors)) {
    throw new CommerceApiError(
      "decode",
      operation,
      "response_invalid",
    );
  }
  return {
    data: record["data"],
    errors: errors.map((error) => {
      const item = getRequiredRecord(error, "GraphQL error");
      const path = parseGraphqlPath(item["path"], operation);
      getRequiredString(item["message"], "GraphQL error message");
      return {
        code: "graphql_field_error",
        ...(path === undefined ? {} : { path }),
      };
    }),
  };
}

function parseGraphqlPath(
  value: unknown,
  operation: string,
): readonly GraphqlPathPart[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value)) {
    throw new CommerceApiError(
      "decode",
      operation,
      "response_invalid",
    );
  }
  return value.map((part) => {
    if (
      (typeof part !== "string" || part.length === 0) &&
      (typeof part !== "number" || !Number.isSafeInteger(part))
    ) {
      throw new CommerceApiError(
        "decode",
        operation,
        "response_invalid",
      );
    }
    return part;
  });
}

function parseProduct(value: unknown): Product {
  const record = getRequiredRecord(value, "product");
  return {
    id: getRequiredString(record["id"], "product id"),
    tenantId: getRequiredString(record["tenantId"], "product tenant id"),
    slug: getRequiredString(record["slug"], "product slug"),
    sku: getRequiredString(record["sku"], "product sku"),
    name: getRequiredString(record["name"], "product name"),
    description: getRequiredString(
      record["description"],
      "product description",
    ),
    brand: getOptionalString(record["brand"]),
    category: parseCategory(record["category"]),
    priceMinor: getOptionalInt(record["priceMinor"]),
    price: parseOptional(record["price"], parsePriceSnapshot),
    variants: getRequiredArray(record["variants"], "product variants").map(
      parseVariant,
    ),
    reviews: getRequiredArray(record["reviews"], "product reviews").map(
      parseReview,
    ),
    reviewsSlow: getRequiredArray(
      record["reviewsSlow"],
      "slow product reviews",
    ).map(parseReview),
    riskScore: getOptionalNumber(record["riskScore"]),
  };
}

function parseCategory(value: unknown): Category {
  const record = getRequiredRecord(value, "category");
  return {
    id: getRequiredString(record["id"], "category id"),
    tenantId: getRequiredString(record["tenantId"], "category tenant id"),
    slug: getRequiredString(record["slug"], "category slug"),
    name: getRequiredString(record["name"], "category name"),
  };
}

function parsePriceSnapshot(value: unknown): PriceSnapshot {
  const record = getRequiredRecord(value, "price snapshot");
  return {
    id: getRequiredString(record["id"], "price id"),
    currency: getRequiredString(record["currency"], "price currency"),
    amountMinor: getRequiredInt(record["amountMinor"], "price amount"),
    compareAtMinor: getOptionalInt(record["compareAtMinor"]),
    validFrom: getRequiredString(record["validFrom"], "price validity").trim(),
  };
}

function parseVariant(value: unknown): ProductVariant {
  const record = getRequiredRecord(value, "product variant");
  return {
    id: getRequiredString(record["id"], "variant id"),
    tenantId: getRequiredString(record["tenantId"], "variant tenant id"),
    productId: getRequiredString(record["productId"], "variant product id"),
    sku: getRequiredString(record["sku"], "variant sku"),
    name: getRequiredString(record["name"], "variant name"),
    options: getRequiredString(record["options"], "variant options"),
    price: parseOptional(record["price"], parsePriceSnapshot),
  };
}

function parseReview(value: unknown): Review {
  const record = getRequiredRecord(value, "review");
  return {
    id: getRequiredString(record["id"], "review id"),
    productId: getRequiredString(record["productId"], "review product id"),
    title: getRequiredString(record["title"], "review title"),
    text: getRequiredString(record["text"], "review text"),
    stars: getRequiredInt(record["stars"], "review stars"),
    verifiedPurchase: getRequiredBoolean(
      record["verifiedPurchase"],
      "review verified flag",
    ),
    createdAt: getRequiredString(record["createdAt"], "review created time"),
  };
}

function parseQuote(value: Record<string, unknown>): Quote {
  return {
    quoteId: getRequiredString(value["quoteId"], "quote id"),
    status: getRequiredString(value["status"], "quote status"),
    lines: getRequiredArray(value["lines"], "quote lines").map(parseQuoteLine),
    subtotal: parseOptional(value["subtotal"], parseMoney),
    discountTotal: parseOptional(value["discountTotal"], parseMoney),
    taxTotal: parseOptional(value["taxTotal"], parseMoney),
    grandTotal: parseOptional(value["grandTotal"], parseMoney),
    validForSeconds: getRequiredInt(value["validForSeconds"], "quote validity"),
    pricingVersion: getRequiredString(
      value["pricingVersion"],
      "pricing version",
    ),
  };
}

function parseQuoteLine(value: unknown): QuoteLine {
  const record = getRequiredRecord(value, "quote line");
  return {
    sku: getRequiredString(record["sku"], "quote line sku"),
    quantity: getRequiredInt(record["quantity"], "quote line quantity"),
    unitPrice: parseOptional(record["unitPrice"], parseMoney),
    lineTotal: parseOptional(record["lineTotal"], parseMoney),
  };
}

function parseMoney(value: unknown): Money {
  const record = getRequiredRecord(value, "money");
  return {
    currencyCode: getRequiredString(record["currencyCode"], "money currency"),
    amountMinor: getNumericInt(record["amountMinor"], "money amount"),
  };
}

function parseCheckoutReceipt(value: unknown): CheckoutReceipt {
  const record = getRequiredRecord(value, "checkout receipt");
  return {
    status: getRequiredString(record["status"], "checkout status"),
    orderId: getOptionalString(record["orderId"]),
    orderNumber: null,
    tenantId: null,
    customerId: null,
    currency: getOptionalString(record["currency"]),
    subtotalMinor: null,
    discountMinor: null,
    totalMinor: getOptionalNumericInt(record["totalMinor"]),
    paymentId: null,
    paymentStatus: getOptionalString(record["paymentStatus"]),
    featureVariant: getOptionalString(record["featureVariant"]),
  };
}

function parseOrderSummary(value: unknown): OrderSummary {
  const record = getRequiredRecord(value, "order summary");
  return {
    id: getRequiredString(record["id"], "order id"),
    orderNumber: getRequiredString(record["order_number"], "order number"),
    status: getRequiredString(record["status"], "order status"),
    currency: getRequiredString(record["currency"], "order currency"),
    totalMinor: getNumericInt(record["total_minor"], "order total"),
    createdAt: getOptionalTimestamp(record["created_at"]),
  };
}

function parseOrderDetail(value: unknown): OrderDetail {
  const record = getRequiredRecord(value, "order detail");
  const summary = parseOrderSummary(record);
  return {
    ...summary,
    customerId: getOptionalString(record["customer_id"]),
    paymentStatus: getOptionalString(record["payment_status"]),
    subtotalMinor: getNumericInt(record["subtotal_minor"], "order subtotal"),
    discountMinor: getNumericInt(record["discount_minor"], "order discount"),
    taxMinor: getNumericInt(record["tax_minor"], "order tax"),
    shippingMinor: getNumericInt(record["shipping_minor"], "order shipping"),
    items: getRequiredArray(record["items"], "order items").map(parseOrderItem),
  };
}

function parseOrderItem(value: unknown): OrderItem {
  const record = getRequiredRecord(value, "order item");
  return {
    sku: getRequiredString(record["sku"], "order item sku"),
    productName: getRequiredString(
      record["product_name"],
      "order item product name",
    ),
    quantity: getNumericInt(record["quantity"], "order item quantity"),
    unitPriceMinor: getNumericInt(
      record["unit_price_minor"],
      "order item unit price",
    ),
    discountMinor: getNumericInt(
      record["discount_minor"],
      "order item discount",
    ),
    lineTotalMinor: getNumericInt(
      record["line_total_minor"],
      "order item total",
    ),
  };
}

function parseAnalyticsEvent(value: unknown): AnalyticsEvent {
  const record = getRequiredRecord(value, "analytics event");
  return {
    eventId: getRequiredString(record["eventId"], "analytics event id"),
    tenantId: getRequiredString(record["tenantId"], "analytics tenant id"),
    eventKey: getRequiredString(record["eventKey"], "analytics event key"),
    customerId: getOptionalString(record["customerId"]),
    eventName: getRequiredString(record["eventName"], "analytics event name"),
    eventVersion: getRequiredInt(
      record["eventVersion"],
      "analytics event version",
    ),
    source: getRequiredString(record["source"], "analytics source"),
    entityType: getRequiredString(
      record["entityType"],
      "analytics entity type",
    ),
    entityId: getRequiredString(record["entityId"], "analytics entity id"),
    occurredAt: getRequiredString(
      record["occurredAt"],
      "analytics occurred time",
    ),
    traceId: getRequiredString(record["traceId"], "analytics trace id"),
    spanId: getRequiredString(record["spanId"], "analytics span id"),
    properties: getRequiredString(record["properties"], "analytics properties"),
  };
}

function parseAnalyticsSummary(value: unknown): AnalyticsSummary {
  const record = getRequiredRecord(value, "analytics summary");
  return {
    tenantId: getRequiredString(
      record["tenantId"],
      "analytics summary tenant id",
    ),
    eventName: getOptionalString(record["eventName"]),
    eventCount: getRequiredInt(record["eventCount"], "analytics event count"),
    uniqueCustomers: getRequiredInt(
      record["uniqueCustomers"],
      "analytics customer count",
    ),
    firstOccurredAt: getOptionalString(record["firstOccurredAt"]),
    lastOccurredAt: getOptionalString(record["lastOccurredAt"]),
  };
}

function parseOptional<T>(
  value: unknown,
  parse: (value: unknown) => T,
): T | null {
  return value === null || value === undefined ? null : parse(value);
}

function getRequiredRecord(
  value: unknown,
  _label: string,
): Record<string, unknown> {
  if (!isRecord(value))
    throw new CommerceApiError("decode", "response", "response_invalid");
  return value;
}

function getRequiredArray(value: unknown, _label: string): readonly unknown[] {
  if (!Array.isArray(value))
    throw new CommerceApiError("decode", "response", "response_invalid");
  return value;
}

function getRequiredString(value: unknown, _label: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new CommerceApiError("decode", "response", "response_invalid");
  }
  return value;
}

function getOptionalString(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") {
    throw new CommerceApiError(
      "decode",
      "response",
      "response_invalid",
    );
  }
  return value;
}

function getRequiredInt(value: unknown, _label: string): number {
  if (!isSafeInteger(value)) {
    throw new CommerceApiError("decode", "response", "response_invalid");
  }
  return value;
}

function getNumericInt(value: unknown, _label: string): number {
  const parsed = typeof value === "string" ? Number(value) : value;
  if (!isSafeInteger(parsed)) {
    throw new CommerceApiError("decode", "response", "response_invalid");
  }
  return parsed;
}

function getOptionalInt(value: unknown): number | null {
  if (value === null || value === undefined) return null;
  if (!isSafeInteger(value)) {
    throw new CommerceApiError(
      "decode",
      "response",
      "response_invalid",
    );
  }
  return value;
}

function getOptionalNumericInt(value: unknown): number | null {
  if (value === null || value === undefined) return null;
  const parsed = typeof value === "string" ? Number(value) : value;
  if (!isSafeInteger(parsed)) {
    throw new CommerceApiError(
      "decode",
      "response",
      "response_invalid",
    );
  }
  return parsed;
}

function getOptionalNumber(value: unknown): number | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new CommerceApiError(
      "decode",
      "response",
      "response_invalid",
    );
  }
  return value;
}

function isSafeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value);
}

function getRequiredBoolean(value: unknown, _label: string): boolean {
  if (typeof value !== "boolean") {
    throw new CommerceApiError("decode", "response", "response_invalid");
  }
  return value;
}

function getOptionalTimestamp(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value)) {
    return new Date(value * 1000).toISOString();
  }
  throw new CommerceApiError(
    "decode",
    "response",
    "response_invalid",
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function formatMoney(
  amountMinor: number | null | undefined,
  currency: string = DEMO_CURRENCY,
): string {
  if (amountMinor === null || amountMinor === undefined)
    return "Price unavailable";
  try {
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency,
    }).format(amountMinor / 100);
  } catch {
    return `${currency} ${(amountMinor / 100).toFixed(2)}`;
  }
}

export function displayDate(value: string | null): string {
  if (value === null) return "Date unavailable";
  const numeric = Number(value);
  const date =
    Number.isFinite(numeric) && value.trim() !== ""
      ? new Date(numeric * 1000)
      : new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat("en-US", {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(date);
}

export function errorMessageForUser(error: unknown): string {
  reportHandledError(
    error,
    error instanceof CommerceApiError ? error.operation : "web.ui",
  );
  return safeWebMessage(error);
}

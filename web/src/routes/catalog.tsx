import {
  Link,
  createFileRoute,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { useCart } from "../cart";
import {
  type CatalogSort,
  DEMO_CUSTOMER_ID,
  DEMO_TENANT_ID,
  errorMessageForUser,
  fetchCategories,
  fetchProducts,
  recordAnalytics,
  type Category,
  type ProductPage,
} from "../commerce";
import { LoadingBlock, Notice, PageFrame, ProductCard } from "../components";
import {
  emitTypedEvent,
  reportHandledError,
  runTracedStep,
  trackStep,
} from "../rum";
import {
  APP_SCREEN_NAME,
  APP_WIDGET_NAME,
  UI_CLICK,
  UI_ACTION_CART_ADD,
} from "../semconv";

export const Route = createFileRoute("/catalog")({
  validateSearch: (search: Record<string, unknown>) => ({
    search:
      typeof search["search"] === "string" && search["search"].trim() !== ""
        ? search["search"].trim()
        : undefined,
    category:
      typeof search["category"] === "string" && search["category"].trim() !== ""
        ? search["category"].trim()
        : undefined,
    sort:
      typeof search["sort"] === "string"
        ? parseCatalogSort(search["sort"])
        : undefined,
    page: parseCatalogPage(search["page"]),
  }),
  loaderDeps: ({ search }) => ({
    query: search.search ?? "",
    category: search.category ?? null,
    sort: search.sort ?? DEFAULT_CATALOG_SORT,
    page: search.page ?? 0,
  }),
  loader: async ({ deps }) => {
    const [products, categories] = await Promise.allSettled([
      fetchProducts({
        tenantId: DEMO_TENANT_ID,
        search: deps.query || undefined,
        category: deps.category ?? undefined,
        sort: deps.sort,
        page: deps.page,
        size: CATALOG_PAGE_SIZE,
      }),
      fetchCategories(DEMO_TENANT_ID),
    ]);
    return {
      products: products.status === "fulfilled" ? products.value : null,
      productsError:
        products.status === "rejected"
          ? errorMessageForUser(products.reason)
          : null,
      categories: categories.status === "fulfilled" ? categories.value : null,
      categoriesError:
        categories.status === "rejected"
          ? errorMessageForUser(categories.reason)
          : null,
    } as const;
  },
  component: CatalogPage,
});

type LoadState<T> =
  | Readonly<{ kind: "loading" }>
  | Readonly<{ kind: "ready"; data: T }>
  | Readonly<{ kind: "error"; message: string }>;

const CATALOG_PAGE_SIZE = 20;
const DEFAULT_CATALOG_SORT: CatalogSort = "FEATURED";
const SORT_OPTIONS: readonly Readonly<{
  value: CatalogSort;
  label: string;
}>[] = [
  { value: "FEATURED", label: "Featured" },
  { value: "RELEVANCE", label: "Relevance" },
  { value: "NEWEST", label: "Newest" },
  { value: "PRICE_ASC", label: "Price: low to high" },
  { value: "PRICE_DESC", label: "Price: high to low" },
];

function CatalogPage() {
  const { addItem } = useCart();
  const navigate = useNavigate({ from: "/catalog" });
  const router = useRouter();
  const { search = "", category = null, sort = DEFAULT_CATALOG_SORT, page = 0 } =
    Route.useSearch();
  const loaderData = Route.useLoaderData();
  const [searchInput, setSearchInput] = useState(search);
  const [fallbackProducts, setFallbackProducts] = useState<ProductPage | null>(
    loaderData.products,
  );
  const [fallbackCategories, setFallbackCategories] = useState<
    readonly Category[] | null
  >(loaderData.categories);
  const [addedSku, setAddedSku] = useState<string | null>(null);

  useEffect(() => setSearchInput(search), [search]);
  useEffect(() => {
    if (loaderData.products !== null) {
      setFallbackProducts(loaderData.products);
      return;
    }
    const controller = new AbortController();
    setFallbackProducts(null);
    void fetchProducts({
      tenantId: DEMO_TENANT_ID,
      search: search || undefined,
      category: category ?? undefined,
      sort,
      page,
      size: CATALOG_PAGE_SIZE,
      signal: controller.signal,
    }).then(
      (products) => {
        if (!controller.signal.aborted) setFallbackProducts(products);
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [category, loaderData.products, page, search, sort]);
  useEffect(() => {
    if (loaderData.categories !== null) {
      setFallbackCategories(loaderData.categories);
      return;
    }
    const controller = new AbortController();
    setFallbackCategories(null);
    void fetchCategories(DEMO_TENANT_ID, controller.signal).then(
      (categories) => {
        if (!controller.signal.aborted) setFallbackCategories(categories);
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [loaderData.categories]);

  const productsState: LoadState<ProductPage> =
    fallbackProducts !== null
      ? { kind: "ready", data: fallbackProducts }
        : loaderData.productsError !== null
          ? { kind: "error", message: loaderData.productsError }
          : { kind: "loading" };
  const categoriesState: LoadState<readonly Category[]> =
    fallbackCategories !== null
      ? { kind: "ready", data: fallbackCategories }
      : loaderData.categoriesError !== null
        ? { kind: "error", message: loaderData.categoriesError }
        : { kind: "loading" };

  function resetBrowse() {
    setSearchInput("");
    void navigate({
      search: (previous) => ({
        ...previous,
        search: undefined,
        category: undefined,
        sort: undefined,
        page: undefined,
      }),
    });
  }

  async function submitSearch(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runTracedStep(
      UI_CLICK,
      {
        [APP_SCREEN_NAME]: "catalog",
        [APP_WIDGET_NAME]: "catalog-search",
        query_length: searchInput.trim().length,
      },
      async () => undefined,
    );
    await navigate({
      search: (previous) => ({
        ...previous,
        search: searchInput.trim() || undefined,
        page: undefined,
      }),
    });
  }

  function selectCategory(nextCategory: string | null) {
    void navigate({
      search: (previous) => ({
        ...previous,
        category: nextCategory ?? undefined,
        page: undefined,
      }),
    });
    void trackStep(UI_CLICK, {
      [APP_SCREEN_NAME]: "catalog",
      [APP_WIDGET_NAME]: "category-filter",
      category: nextCategory ?? "all",
    });
  }

  function selectSort(nextSort: CatalogSort) {
    void navigate({
      search: (previous) => ({
        ...previous,
        sort: nextSort,
        page: undefined,
      }),
    });
    void trackStep(UI_CLICK, {
      [APP_SCREEN_NAME]: "catalog",
      [APP_WIDGET_NAME]: "catalog-sort",
      sort: nextSort,
    });
  }

  function selectPage(nextPage: number) {
    void navigate({
      search: (previous) => ({ ...previous, page: nextPage }),
    });
    void trackStep(UI_CLICK, {
      [APP_SCREEN_NAME]: "catalog",
      [APP_WIDGET_NAME]: "catalog-pagination",
      page: nextPage + 1,
    });
  }

  async function addToCart(sku: string) {
    await runTracedStep(
      UI_CLICK,
      {
        [APP_SCREEN_NAME]: "catalog",
        [APP_WIDGET_NAME]: UI_ACTION_CART_ADD,
        sku,
      },
      async () => {
        if (!addItem(sku)) return;
        setAddedSku(sku);
        await emitTypedEvent("web.cart.item_added", { sku, source: "catalog" });
        await recordCatalogEvent(sku);
      },
    );
  }

  return (
    <PageFrame
      eyebrow="catalog / GraphQL read"
      title="Browse the real assortment."
      description="Products, variants, categories, reviews, and current prices are loaded from the Catalog service through the Rust storefront gateway."
      actions={
        <Link className="button button-secondary" to="/cart">
          View cart
        </Link>
      }
    >
      <div className="catalog-layout">
        <section
          aria-label="Catalog products"
          aria-busy={productsState.kind === "loading"}
        >
          <div className="catalog-toolbar">
            <form
              className="toolbar"
              onSubmit={(event) => void submitSearch(event)}
            >
              <label className="sr-only" htmlFor="catalog-search">
                Search products
              </label>
              <input
                className="search-field"
                id="catalog-search"
                value={searchInput}
                onChange={(event) => setSearchInput(event.target.value)}
                placeholder="Search by product or SKU"
              />
              <button className="button button-small" type="submit">
                Search
              </button>
              {search ? (
                <button
                  className="button button-small button-quiet"
                  type="button"
                  onClick={resetBrowse}
                >
                  Clear
                </button>
              ) : null}
            </form>
            <label className="catalog-sort-control" htmlFor="catalog-sort">
              <span>Sort by</span>
              <select
                className="field select-field"
                id="catalog-sort"
                value={sort}
                onChange={(event) => {
                  const nextSort = parseCatalogSort(event.currentTarget.value);
                  if (nextSort !== undefined) selectSort(nextSort);
                }}
              >
                {SORT_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {productsState.kind === "loading" ? (
            <LoadingBlock label="Loading catalog" />
          ) : null}
          {productsState.kind === "error" ? (
            <Notice
              tone="error"
              title="Catalog read failed"
              action={
                <button
                  className="button button-small button-secondary"
                  type="button"
                  onClick={() => void router.invalidate()}
                >
                  Retry
                </button>
              }
            >
              {productsState.message}
            </Notice>
          ) : null}
          {productsState.kind === "ready" &&
          productsState.data.items.length === 0 ? (
            <div className="empty-state">
              <h2>No catalog matches</h2>
              <p>
                Try another search or reset the server-owned category and sort
                controls. The UI never invents a fallback product.
              </p>
              <button
                className="button button-secondary"
                type="button"
                onClick={resetBrowse}
              >
                Reset browse
              </button>
            </div>
          ) : null}
          {productsState.kind === "ready" && productsState.data.items.length > 0 ? (
            <>
              <div className="split-row catalog-result-meta">
                <span className="muted">
                  {productsState.data.totalElements} products
                </span>
                <span className="muted">
                  page {productsState.data.page + 1} of{" "}
                  {productsState.data.totalPages} · experience:{" "}
                  {productsState.data.experience}
                </span>
              </div>
              <div className="product-grid">
                {productsState.data.items.map((product) => (
                  <ProductCard
                    key={product.id}
                    product={product}
                    onAdd={(sku) => void addToCart(sku)}
                    added={addedSku === product.sku}
                  />
                ))}
              </div>
              <nav className="catalog-pagination" aria-label="Catalog pages">
                <button
                  className="button button-small button-secondary"
                  type="button"
                  disabled={productsState.data.page === 0}
                  onClick={() => selectPage(productsState.data.page - 1)}
                >
                  Previous
                </button>
                <span aria-live="polite">
                  Page {productsState.data.page + 1} of{" "}
                  {productsState.data.totalPages}
                </span>
                <button
                  className="button button-small button-secondary"
                  type="button"
                  disabled={!productsState.data.hasNext}
                  onClick={() => selectPage(productsState.data.page + 1)}
                >
                  Next
                </button>
              </nav>
            </>
          ) : null}
        </section>

        <aside
          className="surface-card catalog-sidebar"
          aria-label="Catalog filters"
        >
          <p className="eyebrow">server-owned filters</p>
          <h2>Departments</h2>
          <p className="section-kicker">
            Category names and result totals come from Catalog. This page never
            filters a fetched page in the browser.
          </p>
          {categoriesState.kind === "loading" ? (
            <LoadingBlock label="Loading filters" />
          ) : null}
          {categoriesState.kind === "error" ? (
            <Notice
              tone="error"
              title="Category read failed"
              action={
                <button
                  className="button button-small button-secondary"
                  type="button"
                  onClick={() => void router.invalidate()}
                >
                  Retry
                </button>
              }
            >
              {categoriesState.message}
            </Notice>
          ) : null}
          {categoriesState.kind === "ready" ? (
            <ul className="filter-list">
              <li>
                <button
                  className={`filter-button${category === null ? " filter-button-active" : ""}`}
                  type="button"
                  aria-pressed={category === null}
                  onClick={() => selectCategory(null)}
                >
                  <span>All products</span>
                </button>
              </li>
              {categoriesState.data.map((item) => (
                <li key={item.id}>
                  <button
                    className={`filter-button${category === item.slug ? " filter-button-active" : ""}`}
                    type="button"
                    aria-pressed={category === item.slug}
                    onClick={() => selectCategory(item.slug)}
                  >
                    <span>{item.name}</span>
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
        </aside>
      </div>
    </PageFrame>
  );
}

function parseCatalogSort(value: string): CatalogSort | undefined {
  switch (value) {
    case "FEATURED":
    case "NEWEST":
    case "PRICE_ASC":
    case "PRICE_DESC":
    case "RELEVANCE":
      return value;
    default:
      return undefined;
  }
}

function parseCatalogPage(value: unknown): number | undefined {
  const page =
    typeof value === "number"
      ? value
      : typeof value === "string" && value.trim() !== ""
        ? Number(value)
        : undefined;
  return typeof page === "number" && Number.isSafeInteger(page) && page >= 0
    ? page
    : undefined;
}

async function recordCatalogEvent(sku: string): Promise<void> {
  try {
    await recordAnalytics({
      eventKey: `web-catalog-${sku}-${crypto.randomUUID()}`,
      eventName: "web.product.added_to_cart",
      entityType: "product_variant",
      entityId: sku,
      tenantId: DEMO_TENANT_ID,
      customerId: DEMO_CUSTOMER_ID,
      properties: { source: "catalog" },
    });
  } catch (error: unknown) {
    reportHandledError(error, "RecordCatalogAnalytics", {
      [APP_SCREEN_NAME]: "catalog",
      [APP_WIDGET_NAME]: "analytics-write",
    });
    void trackStep("web.analytics.record_failed", {
      [APP_SCREEN_NAME]: "catalog",
      [APP_WIDGET_NAME]: "analytics-write",
      error: error instanceof Error ? error.name : "unknown",
    });
  }
}

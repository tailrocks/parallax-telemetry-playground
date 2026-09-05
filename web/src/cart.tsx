import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { CartEntry } from "./commerce";

const STORAGE_KEY = "parallax.playground.cart.v2";
const CART_CHANGED_EVENT = "parallax-playground-cart-changed";
const MAX_QUANTITY = 99;

type CartContextValue = Readonly<{
  items: readonly CartEntry[];
  cartId: string | null;
  itemCount: number;
  storageError: string | null;
  addItem: (sku: string, quantity?: number) => boolean;
  setQuantity: (sku: string, quantity: number) => void;
  removeItem: (sku: string) => void;
  clear: () => void;
}>;

const CartContext = createContext<CartContextValue | null>(null);

export function CartProvider({ children }: Readonly<{ children: ReactNode }>) {
  const [items, setItems] = useState<readonly CartEntry[]>([]);
  const [cartId, setCartId] = useState<string | null>(null);
  const [storageError, setStorageError] = useState<string | null>(null);

  useEffect(() => {
    const load = () => {
      const result = readCart();
      setItems(result.items);
      setCartId(result.cartId);
      setStorageError(result.error);
    };
    load();
    const sync = () => load();
    window.addEventListener("storage", sync);
    window.addEventListener(CART_CHANGED_EVENT, sync);
    return () => {
      window.removeEventListener("storage", sync);
      window.removeEventListener(CART_CHANGED_EVENT, sync);
    };
  }, []);

  const commit = useCallback((next: readonly CartEntry[]): boolean => {
    const normalized = normalizeCart(next);
    if (typeof window === "undefined") return false;
    const nextCartId =
      normalized.length === 0
        ? null
        : cartId ?? `web-cart-${crypto.randomUUID()}`;
    try {
      window.localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ version: 2, cartId: nextCartId, items: normalized }),
      );
      setStorageError(null);
    } catch {
      // Do not let an unpersisted cart become a false checkout promise.
      setStorageError(
        "Cart storage is unavailable. Enable browser storage before changing the cart.",
      );
      return false;
    }
    setItems(normalized);
    setCartId(nextCartId);
    window.dispatchEvent(new Event(CART_CHANGED_EVENT));
    return true;
  }, [cartId]);

  const addItem = useCallback(
    (sku: string, quantity = 1) => {
      const cleanSku = sku.trim();
      if (
        cleanSku.length === 0 ||
        !Number.isSafeInteger(quantity) ||
        quantity <= 0
      )
        return false;
      const existing = items.find((item) => item.sku === cleanSku);
      return commit(
        existing
          ? items.map((item) =>
              item.sku === cleanSku
                ? {
                    sku: item.sku,
                    quantity: Math.min(MAX_QUANTITY, item.quantity + quantity),
                  }
                : item,
            )
          : [
              ...items,
              { sku: cleanSku, quantity: Math.min(MAX_QUANTITY, quantity) },
            ],
      );
    },
    [commit, items],
  );

  const setQuantity = useCallback(
    (sku: string, quantity: number) => {
      if (!Number.isSafeInteger(quantity)) return;
      commit(
        quantity <= 0
          ? items.filter((item) => item.sku !== sku)
          : items.map((item) =>
              item.sku === sku
                ? { sku: item.sku, quantity: Math.min(MAX_QUANTITY, quantity) }
                : item,
            ),
      );
    },
    [commit, items],
  );

  const removeItem = useCallback(
    (sku: string) => commit(items.filter((item) => item.sku !== sku)),
    [commit, items],
  );

  const clear = useCallback(() => commit([]), [commit]);
  const itemCount = items.reduce((sum, item) => sum + item.quantity, 0);
  const value = useMemo(
    () => ({
      items,
      cartId,
      itemCount,
      storageError,
      addItem,
      setQuantity,
      removeItem,
      clear,
    }),
    [addItem, cartId, clear, itemCount, items, removeItem, setQuantity, storageError],
  );

  return <CartContext.Provider value={value}>{children}</CartContext.Provider>;
}

export function useCart(): CartContextValue {
  const value = useContext(CartContext);
  if (value === null)
    throw new Error("useCart must be used inside CartProvider");
  return value;
}

function readCart(): Readonly<{
  items: readonly CartEntry[];
  cartId: string | null;
  error: string | null;
}> {
  if (typeof window === "undefined") return { items: [], cartId: null, error: null };
  try {
    const raw: unknown = JSON.parse(
      window.localStorage.getItem(STORAGE_KEY) ??
        '{"version":2,"cartId":null,"items":[]}',
    );
    if (!isRecord(raw) || raw["version"] !== 2) {
      throw new Error("stored cart has an unsupported version");
    }
    const storedCartId = raw["cartId"];
    if (
      storedCartId !== null &&
      (typeof storedCartId !== "string" || storedCartId.trim().length === 0)
    ) {
      throw new Error("stored cart has an invalid durable cart id");
    }
    return {
      items: normalizeCart(raw["items"]),
      cartId: storedCartId === null ? null : storedCartId,
      error: null,
    };
  } catch (error: unknown) {
    return {
      items: [],
      cartId: null,
      error: "Cart storage is invalid. Clear the stored cart before continuing.",
    };
  }
}

function normalizeCart(value: unknown): readonly CartEntry[] {
  if (!Array.isArray(value))
    throw new Error("stored cart must be an array");
  const merged = new Map<string, number>();
  for (const entry of value) {
    if (!isRecord(entry)) throw new Error("stored cart contains an invalid item");
    const sku = entry["sku"];
    const quantity = entry["quantity"];
    if (
      typeof sku !== "string" ||
      sku.trim().length === 0 ||
      typeof quantity !== "number" ||
      !Number.isSafeInteger(quantity) ||
      quantity <= 0
    )
      throw new Error("stored cart contains an invalid SKU or quantity");
    merged.set(
      sku.trim(),
      Math.min(MAX_QUANTITY, (merged.get(sku.trim()) ?? 0) + quantity),
    );
  }
  return [...merged.entries()].map(([sku, quantity]) => ({ sku, quantity }));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

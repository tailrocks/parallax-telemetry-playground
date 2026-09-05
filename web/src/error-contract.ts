export const SAFE_WEB_ERROR_MESSAGES = {
  network_unavailable: "The storefront is unavailable. Retry.",
  http_failure: "The storefront request failed. Retry.",
  graphql_failure: "The storefront could not complete this request. Retry.",
  response_invalid: "The storefront returned an invalid response. Retry.",
  unexpected: "Something unexpected happened. Retry.",
} as const;

export type SafeWebErrorCode = keyof typeof SAFE_WEB_ERROR_MESSAGES;

const SAFE_WEB_ERROR_CODES = new Set<SafeWebErrorCode>(
  Object.keys(SAFE_WEB_ERROR_MESSAGES) as SafeWebErrorCode[],
);
const SAFE_ERROR_ATTRIBUTE_KEYS = new Set([
  "error_kind",
  "http_status",
  "graphql_error_count",
  "graphql_error_paths",
  "app.screen.name",
  "app.widget.name",
]);

/** Translate unknown boundary failures into stable, non-sensitive web errors. */
export function safeWebError(error: unknown): Error {
  const code = safeWebErrorCode(error);
  const normalized = new Error(SAFE_WEB_ERROR_MESSAGES[code]);
  normalized.name = code;
  return normalized;
}

export function safeWebErrorCode(error: unknown): SafeWebErrorCode {
  if (!isRecord(error)) return "unexpected";
  const code = error["code"];
  return typeof code === "string" && SAFE_WEB_ERROR_CODES.has(code as SafeWebErrorCode)
    ? (code as SafeWebErrorCode)
    : "unexpected";
}

export function safeWebMessage(error: unknown): string {
  return safeWebError(error).message;
}

export function safeWebOperation(value: string, fallback = "web.ui"): string {
  return /^[A-Za-z][A-Za-z0-9_.:-]{0,95}$/.test(value) ? value : fallback;
}

export function safeWebAttributes(
  attributes: Readonly<Record<string, string | number | boolean | undefined>>,
): Record<string, string | number | boolean> {
  const safe: Record<string, string | number | boolean> = {};
  for (const [key, value] of Object.entries(attributes)) {
    if (!SAFE_ERROR_ATTRIBUTE_KEYS.has(key) || value === undefined) continue;
    if (typeof value === "number") {
      if (Number.isSafeInteger(value)) safe[key] = value;
      continue;
    }
    if (typeof value === "boolean") {
      safe[key] = value;
      continue;
    }
    if (/^[A-Za-z0-9_.:/,<>-]{1,256}$/.test(value)) safe[key] = value;
  }
  return safe;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

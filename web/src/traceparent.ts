import type { Baggage, BaggageEntry } from "@opentelemetry/api";

const TRACEPARENT_PATTERN =
  /^00-([0-9a-f]{32})-([0-9a-f]{16})-([0-9a-f]{2})$/;
const ZERO_TRACE_ID = /^0{32}$/;
const ZERO_SPAN_ID = /^0{16}$/;
const TRACESTATE_KEY_PATTERN = /^[a-z0-9][a-z0-9_*/-]{0,255}$/;
const TRACESTATE_VENDOR_KEY_PATTERN =
  /^[a-z0-9][a-z0-9_*/-]{0,240}@[a-z0-9][a-z0-9_*/-]{0,13}$/;
const TRACESTATE_VALUE_PATTERN =
  /^(?:[\x20-\x2b\x2d-\x3c\x3e-\x7e]{0,255}[\x21-\x2b\x2d-\x3c\x3e-\x7e])$/;
const MAX_TRACESTATE_MEMBERS = 32;
const MAX_TRACESTATE_HEADER_BYTES = 512;
const MAX_TRACESTATE_VALUE_BYTES = 256;
const BAGGAGE_KEY_PATTERN =
  /^[a-z0-9][a-z0-9._-]{0,255}(?:@[a-z0-9][a-z0-9._-]{0,13})?$/;
const ENCODED_BAGGAGE_VALUE_PATTERN =
  /^(?:[\x21-\x2b\x2d-\x3c\x3e-\x7e]|%[0-9a-f]{2})+$/i;
const SAFE_BAGGAGE_KEYS = new Set([
  "tenant.id",
  "user.tier",
  "customer.segment",
  "region",
  "request.priority",
  "feature.variant",
  "session.id",
  "cli.invocation.id",
]);
const MAX_BAGGAGE_ENTRIES = 32;
const MAX_BAGGAGE_VALUE_LENGTH = 128;
const MAX_BAGGAGE_METADATA_LENGTH = 128;
const MAX_BAGGAGE_HEADER_LENGTH = 2048;

export type PropagationHeaders = Readonly<{
  traceparent?: string | undefined;
  tracestate?: string | undefined;
  baggage?: string | undefined;
}>;

/**
 * Accept only a trace context supplied by the page request's caller.
 *
 * A server span is created by `server-telemetry.ts`; this helper only validates
 * wire input and never mints a traceparent.
 */
export function validTraceparent(value: string | null): string | undefined {
  if (value === null || value.length === 0) return undefined;
  const match = TRACEPARENT_PATTERN.exec(value);
  if (match === null) return undefined;
  const [, traceId, spanId, flags] = match;
  if (
    traceId === undefined ||
    spanId === undefined ||
    flags === undefined ||
    ZERO_TRACE_ID.test(traceId) ||
    ZERO_SPAN_ID.test(spanId)
  ) {
    return undefined;
  }
  return value;
}

/** Convert the genuine SSR W3C span into Sentry's pageload parent format. */
export function sentryTraceFromTraceparent(
  value: string | null,
): string | undefined {
  const normalized = validTraceparent(value);
  if (normalized === undefined) return undefined;
  const [, traceId, spanId, flags] = normalized.split("-");
  return `${traceId}-${spanId}-${Number.parseInt(flags ?? "0", 16) & 1}`;
}

/** Validate the bounded W3C tracestate value before it reaches HTML or OTel. */
export function validTracestate(value: string | null): string | undefined {
  if (value === null || value.length === 0) return undefined;
  if (new TextEncoder().encode(value).byteLength > MAX_TRACESTATE_HEADER_BYTES) {
    return undefined;
  }

  const rawMembers = value.split(",");
  if (rawMembers.length > MAX_TRACESTATE_MEMBERS) {
    return undefined;
  }
  const members: string[] = [];
  const seen = new Set<string>();
  for (const rawMember of rawMembers) {
    const member = trimTracestateOws(rawMember);
    if (member.length === 0) return undefined;
    if (!validTracestateMember(member)) return undefined;
    const key = member.slice(0, member.indexOf("="));
    if (seen.has(key)) return undefined;
    seen.add(key);
    members.push(member);
  }
  return members.join(",");
}

function validTracestateMember(member: string): boolean {
  const separator = member.indexOf("=");
  if (separator <= 0 || separator === member.length - 1) return false;
  const key = member.slice(0, separator);
  const value = member.slice(separator + 1);
  return (
    (TRACESTATE_KEY_PATTERN.test(key) ||
      TRACESTATE_VENDOR_KEY_PATTERN.test(key)) &&
    new TextEncoder().encode(value).byteLength <= MAX_TRACESTATE_VALUE_BYTES &&
    TRACESTATE_VALUE_PATTERN.test(value)
  );
}

function trimTracestateOws(value: string): string {
  return value.replace(/^[ \t]+|[ \t]+$/g, "");
}

/**
 * Keep every inherited business baggage entry that is safe and bounded. The
 * allowlist prevents credentials or arbitrary request data from entering the
 * browser document and downstream services.
 */
export function boundedBaggageEntries(
  baggage: Baggage | undefined,
): Record<string, BaggageEntry> {
  const entries: Record<string, BaggageEntry> = {};
  if (baggage === undefined) return entries;

  for (const [key, entry] of baggage.getAllEntries()) {
    if (Object.keys(entries).length >= MAX_BAGGAGE_ENTRIES) break;
    if (!validBaggageKey(key) || !validBaggageValue(entry.value)) continue;
    const metadata = entry.metadata?.toString();
    if (metadata !== undefined && !validBaggageMetadata(metadata)) continue;
    if (entry.metadata === undefined) {
      entries[key] = { value: entry.value };
    } else {
      entries[key] = { value: entry.value, metadata: entry.metadata };
    }
  }
  return entries;
}

/** Sanitize a raw W3C baggage header while retaining all valid safe members. */
export function sanitizeBaggageHeader(value: string | null): string | undefined {
  const normalized = value?.trim();
  if (normalized === undefined || normalized.length === 0) return undefined;
  if (normalized.length > MAX_BAGGAGE_HEADER_LENGTH) return undefined;

  const members: string[] = [];
  const seen = new Set<string>();
  for (const rawMember of normalized.split(",")) {
    if (members.length >= MAX_BAGGAGE_ENTRIES) break;
    const member = rawMember.trim();
    const separator = member.indexOf("=");
    if (separator <= 0) continue;
    const key = member.slice(0, separator).trim();
    if (seen.has(key) || !validBaggageKey(key)) continue;

    const valueAndMetadata = member.slice(separator + 1).split(";");
    const encodedValue = valueAndMetadata.shift()?.trim() ?? "";
    if (!validEncodedBaggageValue(encodedValue)) continue;
    const metadata = valueAndMetadata.join(";").trim();
    if (metadata.length > 0 && !validBaggageMetadata(metadata)) continue;

    seen.add(key);
    members.push(
      `${key}=${encodedValue}${metadata.length > 0 ? `;${metadata}` : ""}`,
    );
  }
  return members.length > 0 ? members.join(",") : undefined;
}

export function sanitizePropagationHeaders(
  headers: PropagationHeaders,
): PropagationHeaders {
  return {
    traceparent: validTraceparent(headers.traceparent ?? null),
    tracestate: validTracestate(headers.tracestate ?? null),
    baggage: sanitizeBaggageHeader(headers.baggage ?? null),
  };
}

/** Sanitize OTel output before a request crosses the browser/server boundary. */
export function sanitizePropagationHeadersInPlace(headers: Headers): void {
  const sanitized = sanitizePropagationHeaders({
    traceparent: headers.get("traceparent") ?? undefined,
    tracestate: headers.get("tracestate") ?? undefined,
    baggage: headers.get("baggage") ?? undefined,
  });
  for (const [name, value] of Object.entries(sanitized)) {
    if (value === undefined) headers.delete(name);
    else headers.set(name, value);
  }
}

function validBaggageKey(key: string): boolean {
  return SAFE_BAGGAGE_KEYS.has(key) && BAGGAGE_KEY_PATTERN.test(key);
}

function validBaggageValue(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= MAX_BAGGAGE_VALUE_LENGTH &&
    /^[\x21-\x2b\x2d-\x3c\x3e-\x7e]+$/.test(value) &&
    !value.includes(",") &&
    !value.includes(";") &&
    !value.includes("=") &&
    !value.includes("%") &&
    !value.split("").some((character) => character.charCodeAt(0) < 0x20)
  );
}

function validEncodedBaggageValue(value: string): boolean {
  if (value.length === 0 || value.length > MAX_BAGGAGE_VALUE_LENGTH * 3) {
    return false;
  }
  if (!ENCODED_BAGGAGE_VALUE_PATTERN.test(value)) return false;
  let decoded: string;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    return false;
  }
  return validBaggageValue(decoded);
}

function validBaggageMetadata(value: string): boolean {
  if (value.length === 0 || value.length > MAX_BAGGAGE_METADATA_LENGTH) {
    return false;
  }
  return value.split(";").every((property) => {
    const separator = property.indexOf("=");
    if (separator <= 0 || separator === property.length - 1) return false;
    return (
      BAGGAGE_KEY_PATTERN.test(property.slice(0, separator).trim()) &&
      validEncodedBaggageValue(property.slice(separator + 1).trim())
    );
  });
}

/** Add bounded W3C context to an SSR document for browser bootstrap. */
export function injectPropagationMeta(
  html: string,
  headers: PropagationHeaders,
): string {
  let result = html;
  for (const [name, value] of [
    ["traceparent", validTraceparent(headers.traceparent ?? null)],
    ["tracestate", validTracestate(headers.tracestate ?? null)],
    ["baggage", sanitizeBaggageHeader(headers.baggage ?? null)],
    ["sentry-trace", sentryTraceFromTraceparent(headers.traceparent ?? null)],
  ] as const) {
    if (value === undefined) continue;
    const meta = `<meta name="${name}" content="${escapeHtmlAttribute(value)}">`;
    const existing = new RegExp(
      `<meta\\b[^>]*\\bname=["']${name}["'][^>]*>`,
      "i",
    );
    result = existing.test(result)
      ? result.replace(existing, meta)
      : insertMetaIntoHead(result, meta);
  }
  return result;
}

function insertMetaIntoHead(html: string, meta: string): string {
  const head = /<head\b[^>]*>/i;
  return head.test(html)
    ? html.replace(head, (opening) => `${opening}${meta}`)
    : html;
}

function escapeHtmlAttribute(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll('"', "&quot;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

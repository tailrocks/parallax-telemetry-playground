// Mint a fresh W3C `traceparent` (`00-<trace>-<span>-01`). Isomorphic: uses Web
// Crypto, present in the browser, Node 18+, and the Nitro server runtime.
// The root route emits this as <meta name="traceparent"> during SSR so the
// browser's OTel document-load instrumentation can parent the first-paint span
// under the same trace (the documented SSR→browser handoff; §6 of the spec —
// initial navigation has no fetch to inject a header into).
export function mintTraceparent(): string {
  const bytes = new Uint8Array(24);
  crypto.getRandomValues(bytes);
  const hex = (start: number, len: number) =>
    Array.from(bytes.subarray(start, start + len), (b) =>
      b.toString(16).padStart(2, "0"),
    ).join("");
  return `00-${hex(0, 16)}-${hex(16, 8)}-01`;
}

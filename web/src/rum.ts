import type { RumAttributes } from "./telemetry";

export async function trackStep(
  name: string,
  attributes: RumAttributes = {},
) {
  if (typeof window === "undefined") return;
  const telemetry = await import("./telemetry");
  telemetry.trackStep(name, attributes);
}

export async function runTracedStep<T>(
  name: string,
  attributes: RumAttributes,
  fn: () => Promise<T>,
): Promise<T> {
  if (typeof window === "undefined") return fn();
  const telemetry = await import("./telemetry");
  return telemetry.runTracedStep(name, attributes, fn);
}

export async function tracedFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  if (typeof window === "undefined") return fetch(input, init);
  const telemetry = await import("./telemetry");
  return telemetry.tracedFetch(input, init);
}

export async function emitTypedEvent(
  name: string,
  attributes: RumAttributes = {},
) {
  if (typeof window === "undefined") return;
  const telemetry = await import("./telemetry");
  telemetry.emitTypedEvent(name, attributes);
}

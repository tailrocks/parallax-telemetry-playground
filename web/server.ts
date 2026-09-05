import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { extname, join, normalize } from "node:path";
import { safeWebError } from "./src/error-contract";
import { tracedFetch } from "./src/rum";
import {
  shutdownServerTelemetry as shutdownProxyTelemetry,
  traceServerRequest,
} from "./src/server-telemetry";

interface HandlerModule {
  readonly default: {
    readonly fetch: (request: Request) => Response | Promise<Response>;
  };
  readonly shutdownServerTelemetry?: () => Promise<void>;
}

function isHandlerModule(value: unknown): value is HandlerModule {
  if (typeof value !== "object" || value === null || !("default" in value)) {
    return false;
  }
  const handler = value.default;
  return (
    typeof handler === "object" &&
    handler !== null &&
    "fetch" in handler &&
    typeof handler.fetch === "function"
  );
}

const serverEntry = new URL("./dist/server/server.js", import.meta.url).href;
const generatedHandler: unknown = await import(serverEntry);
if (!isHandlerModule(generatedHandler)) {
  throw new TypeError("generated server does not export a fetch handler");
}
const generatedFetch = generatedHandler.default.fetch;
const generatedShutdown = generatedHandler.shutdownServerTelemetry;

const port = Number(process.env["PORT"] ?? 3000);
const host = process.env["HOST"] ?? "0.0.0.0";
const clientDir = new URL("./dist/client/", import.meta.url).pathname;
const playgroundGitSha = (
  process.env["VITE_GIT_SHA"] ??
  process.env["GIT_SHA"] ??
  ""
).trim();
const STOREFRONT_GRAPHQL_PROXY_PATH = "/__storefront/graphql";
const STOREFRONT_ORDERS_PROXY_PATH = "/api/orders";

function injectPlaygroundGitSha(html: string): string {
  if (!playgroundGitSha) {
    return html;
  }
  const tag = `<script>globalThis.__PLAYGROUND_GIT_SHA__=${JSON.stringify(playgroundGitSha)}</script>`;
  return html.includes("</head>")
    ? html.replace("</head>", `${tag}</head>`)
    : `${tag}${html}`;
}

const contentTypes: Readonly<Record<string, string>> = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
};

function clientFile(pathname: string): string | undefined {
  const path = normalize(decodeURIComponent(pathname)).replace(/^\.\.[/\\]/, "");
  const file = join(clientDir, path);
  return file.startsWith(clientDir) && existsSync(file) && statSync(file).isFile()
    ? file
    : undefined;
}

async function requestBody(req: IncomingMessage): Promise<ArrayBuffer | undefined> {
  if (req.method === "GET" || req.method === "HEAD") {
    return undefined;
  }
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : Buffer.from(chunk));
  }
  const joined = Buffer.concat(chunks);
  const bytes = new Uint8Array(joined.length);
  bytes.set(joined);
  return bytes.buffer;
}

async function toFetchRequest(req: IncomingMessage): Promise<Request> {
  const origin = `http://${req.headers.host ?? `localhost:${port}`}`;
  const url = new URL(req.url ?? "/", origin);
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) {
    if (Array.isArray(value)) {
      for (const item of value) headers.append(key, item);
    } else if (value !== undefined) {
      headers.set(key, value);
    }
  }
  const body = await requestBody(req);
  return new Request(url, {
    method: req.method ?? "GET",
    headers,
    ...(body === undefined ? {} : { body }),
  });
}

async function writeFetchResponse(
  res: ServerResponse,
  response: Response,
): Promise<void> {
  res.statusCode = response.status;
  res.statusMessage = response.statusText;
  response.headers.forEach((value, key) => {
    res.setHeader(key, value);
  });
  if (response.body === null) {
    res.end();
    return;
  }
  const reader = response.body.getReader();
  for (;;) {
    const result = await reader.read();
    if (result.done) break;
    res.write(result.value);
  }
  res.end();
}

function isStorefrontProxyPath(pathname: string): boolean {
  return (
    pathname === STOREFRONT_GRAPHQL_PROXY_PATH ||
    pathname === STOREFRONT_ORDERS_PROXY_PATH ||
    pathname.startsWith(`${STOREFRONT_ORDERS_PROXY_PATH}/`)
  );
}

function storefrontProxyTarget(pathname: string, search: string): string {
  const configured =
    process.env["STOREFRONT_URL"]?.trim() ?? "http://localhost:8094/graphql";
  const storefrontUrl = new URL(configured);
  if (pathname === STOREFRONT_GRAPHQL_PROXY_PATH) {
    return storefrontUrl.toString();
  }
  return new URL(`${pathname}${search}`, `${storefrontUrl.origin}/`).toString();
}

async function storefrontReady(): Promise<boolean> {
  const target = new URL(
    "/readyz",
    new URL(
      process.env["STOREFRONT_URL"]?.trim() ?? "http://localhost:8094/graphql",
    ).origin,
  );
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 2_000);
  try {
    const response = await fetch(target, {
      method: "GET",
      headers: { accept: "application/json" },
      signal: controller.signal,
    });
    return response.ok;
  } catch {
    return false;
  } finally {
    clearTimeout(timeout);
  }
}

async function proxyStorefront(request: Request): Promise<Response> {
  const requestUrl = new URL(request.url);
  const target = storefrontProxyTarget(requestUrl.pathname, requestUrl.search);
  const headers = new Headers(request.headers);
  for (const name of ["connection", "content-length", "host", "transfer-encoding"]) {
    headers.delete(name);
  }
  const body =
    request.method === "GET" || request.method === "HEAD"
      ? undefined
      : await request.arrayBuffer();
  const upstream = await tracedFetch(target, {
    method: request.method,
    headers,
    ...(body === undefined ? {} : { body }),
  });
  const responseHeaders = new Headers(upstream.headers);
  responseHeaders.delete("content-length");
  return new Response(upstream.body, {
    status: upstream.status,
    statusText: upstream.statusText,
    headers: responseHeaders,
  });
}

async function handleRequest(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const pathname = new URL(req.url ?? "/", "http://localhost").pathname;
  if (pathname === "/healthz") {
    const ready = await storefrontReady();
    res.statusCode = ready ? 200 : 503;
    res.setHeader("content-type", "application/json; charset=utf-8");
    res.end(
      JSON.stringify({
        status: ready ? "UP" : "DOWN",
        dependencies: { storefront: ready ? "UP" : "DOWN" },
      }),
    );
    return;
  }
  const file = clientFile(pathname);
  if (file !== undefined) {
    res.setHeader(
      "content-type",
      contentTypes[extname(file)] ?? "application/octet-stream",
    );
    createReadStream(file).pipe(res);
    return;
  }

  const request = await toFetchRequest(req);
  const response = isStorefrontProxyPath(pathname)
    ? (await traceServerRequest(request, () => proxyStorefront(request))).response
    : await generatedFetch(request);
  const contentType = response.headers.get("content-type") ?? "";
  if (contentType.includes("text/html") && playgroundGitSha) {
    const html = injectPlaygroundGitSha(await response.text());
    const headers = new Headers(response.headers);
    headers.delete("content-length");
    await writeFetchResponse(
      res,
      new Response(html, {
        status: response.status,
        statusText: response.statusText,
        headers,
      }),
    );
    return;
  }
  await writeFetchResponse(res, response);
}

let telemetryShutdown: Promise<void> | undefined;
function shutdownTelemetry(): Promise<void> {
  telemetryShutdown ??= Promise.all([
    generatedShutdown?.() ?? Promise.resolve(),
    shutdownProxyTelemetry(),
  ])
    .then(() => undefined)
    .catch((error: unknown) => {
      console.error(safeWebError(error));
    });
  return telemetryShutdown;
}

const httpServer = createServer((req, res) => {
  void handleRequest(req, res).catch((error: unknown) => {
    console.error(safeWebError(error));
    res.statusCode = 500;
    res.end("Server Error");
  });
});

let shuttingDown = false;
function shutdown(signal: NodeJS.Signals): void {
  if (shuttingDown) return;
  shuttingDown = true;
  console.log(`Received ${signal}; draining web server`);
  httpServer.close((error?: Error) => {
    if (error !== undefined) {
      console.error(safeWebError(error));
      process.exitCode = 1;
    }
    void shutdownTelemetry();
  });
}

process.once("SIGTERM", () => shutdown("SIGTERM"));
process.once("SIGINT", () => shutdown("SIGINT"));
httpServer.on("close", () => {
  void shutdownTelemetry();
});

httpServer.listen(port, host, () => {
  console.log(`Listening on http://${host}:${port}`);
});

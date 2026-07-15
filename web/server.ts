import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { extname, join, normalize } from "node:path";

interface HandlerModule {
  readonly default: {
    readonly fetch: (request: Request) => Response | Promise<Response>;
  };
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

const port = Number(process.env["PORT"] ?? 3000);
const host = process.env["HOST"] ?? "0.0.0.0";
const clientDir = new URL("./dist/client/", import.meta.url).pathname;

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

createServer((req, res) => {
  void (async () => {
    try {
      const pathname = new URL(req.url ?? "/", "http://localhost").pathname;
      const file = clientFile(pathname);
      if (file !== undefined) {
        res.setHeader(
          "content-type",
          contentTypes[extname(file)] ?? "application/octet-stream",
        );
        createReadStream(file).pipe(res);
        return;
      }
      const response = await generatedHandler.default.fetch(await toFetchRequest(req));
      await writeFetchResponse(res, response);
    } catch (error: unknown) {
      console.error(error);
      res.statusCode = 500;
      res.end("Server Error");
    }
  })();
}).listen(port, host, () => {
  console.log(`Listening on http://${host}:${port}`);
});

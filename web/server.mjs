import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { Readable } from "node:stream";
import handler from "./dist/server/server.js";

const port = Number(process.env.PORT ?? 3000);
const host = process.env.HOST ?? "0.0.0.0";
const clientDir = new URL("./dist/client/", import.meta.url).pathname;

const contentTypes = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
};

function clientFile(pathname) {
  const path = normalize(decodeURIComponent(pathname)).replace(/^(\.\.[/\\])+/, "");
  const file = join(clientDir, path);
  return file.startsWith(clientDir) && existsSync(file) && statSync(file).isFile()
    ? file
    : undefined;
}

async function toFetchRequest(req) {
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
  const body =
    req.method === "GET" || req.method === "HEAD"
      ? undefined
      : Readable.toWeb(req);
  return new Request(url, {
    method: req.method,
    headers,
    body,
    duplex: body ? "half" : undefined,
  });
}

async function writeFetchResponse(res, response) {
  res.statusCode = response.status;
  res.statusMessage = response.statusText;
  response.headers.forEach((value, key) => {
    res.setHeader(key, value);
  });
  if (!response.body) {
    res.end();
    return;
  }
  await Readable.fromWeb(response.body).pipe(res);
}

createServer(async (req, res) => {
  try {
    const pathname = new URL(req.url ?? "/", "http://localhost").pathname;
    const file = clientFile(pathname);
    if (file) {
      res.setHeader(
        "content-type",
        contentTypes[extname(file)] ?? "application/octet-stream"
      );
      createReadStream(file).pipe(res);
      return;
    }
    await writeFetchResponse(res, await handler.fetch(await toFetchRequest(req)));
  } catch (err) {
    console.error(err);
    res.statusCode = 500;
    res.end("Server Error");
  }
}).listen(port, host, () => {
  console.log(`Listening on http://${host}:${port}`);
});

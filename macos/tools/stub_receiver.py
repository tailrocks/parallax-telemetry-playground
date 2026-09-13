#!/usr/bin/env python3
"""Minimal OTLP/HTTP stub receiver. Captures protobuf bodies per signal to a
JSON-lines file for the verify step. No third-party deps."""
import http.server
import json
import sys

OUT = sys.argv[2] if len(sys.argv) > 2 else "/tmp/macotel-stub.jsonl"
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 14318


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def handle_expect_100(self):
        self.send_response_only(100)
        self.end_headers()
        return True

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        with open(OUT, "a") as f:
            f.write(json.dumps({
                "path": self.path,
                "content_type": self.headers.get("Content-Type"),
                "bytes": len(body),
                "b64": __import__("base64").b64encode(body).decode(),
            }) + "\n")
        resp = b"\x00"
        self.send_response(200)
        self.send_header("Content-Type", "application/x-protobuf")
        self.send_header("Content-Length", str(len(resp)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(resp)

    def log_message(self, *a):
        pass


server = http.server.HTTPServer(("127.0.0.1", PORT), Handler)
if PORT == 0:
    print(f"READY {server.server_address[1]}", flush=True)
server.serve_forever()

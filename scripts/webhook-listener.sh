#!/usr/bin/env bash
# Local webhook listener (plan 167): prints every received request line,
# headers, and JSON payload so alert webhook deliveries are capturable as
# evidence. Usage: scripts/webhook-listener.sh [port]  (default 9099)
set -euo pipefail

PORT="${1:-9099}"

echo "webhook-listener: listening on http://127.0.0.1:${PORT} (Ctrl-C to stop)"
python3 - "$PORT" <<'PY'
import json
import sys
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length)
        stamp = datetime.now(timezone.utc).isoformat()
        print(f"\n── webhook {stamp} {self.command} {self.path}")
        for key, value in self.headers.items():
            print(f"   {key}: {value}")
        try:
            print(json.dumps(json.loads(body), indent=2))
        except ValueError:
            print(body.decode("utf-8", "replace"))
        sys.stdout.flush()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *args):  # quiet default access log; we print above
        pass


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
PY

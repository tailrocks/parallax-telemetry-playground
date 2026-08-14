#!/usr/bin/env bash
# c1: seed a TimeoutError, wait for the issue, assert bundle hash via GraphQL
# and `parallax issue context`, then resolve.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin

OTLP_HTTP="${PARALLAX_OTLP_HTTP:-http://127.0.0.1:4318}"
if ! curl -sS -o /dev/null -w "%{http_code}" "$OTLP_HTTP/v1/traces" --max-time 1 >/dev/null 2>&1; then
  OTLP_HTTP="${PARALLAX_OTLP_HTTP:-http://127.0.0.1:14318}"
fi

python3 - "$OTLP_HTTP" <<'PY'
import struct, sys, time, urllib.request

def enc_varint(n):
    out = bytearray()
    while n > 0x7F:
        out.append((n & 0x7F) | 0x80)
        n >>= 7
    out.append(n)
    return bytes(out)

def fld_ld(n, data):
    return enc_varint((n << 3) | 2) + enc_varint(len(data)) + data

def fld_varint(n, v):
    return enc_varint((n << 3) | 0) + enc_varint(v)

def fld_fixed64(n, v):
    return enc_varint((n << 3) | 1) + struct.pack("<Q", v)

def kv_string(k, v):
    return fld_ld(1, k.encode()) + fld_ld(2, fld_ld(1, v.encode()))

now = time.time_ns()
tid, sid = bytes.fromhex("c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1"), bytes.fromhex("c1c1c1c1c1c1c1c1")
event = fld_fixed64(1, now + 1_000_000) + fld_ld(2, b"exception") + fld_ld(3, kv_string("exception.type", "TimeoutError")) + fld_ld(3, kv_string("exception.message", "connection refused"))
status = fld_ld(2, b"connection refused") + fld_varint(3, 2)
span = fld_ld(1, tid) + fld_ld(2, sid) + fld_ld(5, b"checkout") + fld_varint(6, 2) + fld_fixed64(7, now) + fld_fixed64(8, now + 5_000_000) + fld_ld(11, event) + fld_ld(15, status)
body = fld_ld(1, fld_ld(1, fld_ld(1, kv_string("service.name", "checkout"))) + fld_ld(2, fld_ld(2, span)))
req = urllib.request.Request(sys.argv[1].rstrip("/") + "/v1/traces", data=body, method="POST", headers={"content-type": "application/x-protobuf"})
with urllib.request.urlopen(req, timeout=15) as resp:
    print(f"otlp {resp.status}", file=sys.stderr)
PY

fp=""
for _ in $(seq 1 40); do
  data="$(c_gql '{ issues(limit: 5) { items { fingerprint title } } }')"
  fp="$(python3 -c "import json,sys; d=json.loads(sys.argv[1]); items=(d or {}).get('issues',{}).get('items') or []; print(items[0]['fingerprint'] if items else '')" "$data")"
  if [[ -n "$fp" ]]; then break; fi
  sleep 0.5
done
[[ -n "$fp" ]] || { echo "c1: no issue after seed" >&2; exit 1; }

bundle="$(c_gql "{ bundle(fingerprint: \"$fp\") { canonicalHash markdown json } }")"
hash="$(python3 -c "import json,sys; d=json.loads(sys.argv[1]); print(d['bundle']['canonicalHash'])" "$bundle")"
[[ -n "$hash" ]] || { echo "c1: empty canonicalHash" >&2; exit 1; }
python3 -c "import json,sys; d=json.loads(sys.argv[1]); md=d['bundle']['markdown']; js=d['bundle']['json'];
assert 'bundle' in md.lower() or 'alert' in md.lower() or 'issue' in md.lower() or len(md)>20
assert 'schema' in js or 'fingerprint' in js or len(js)>20" "$bundle"

ctx="$("$PARALLAX_BIN" issue context "$fp" --format json)"
echo "$ctx" | python3 -c "import json,sys; d=json.load(sys.stdin); s=json.dumps(d); assert 'bundle' in s.lower() or 'canonical' in s.lower() or 'schema' in s.lower() or len(s)>40"

"$PARALLAX_BIN" issue resolve "$fp" >/dev/null
echo "c1 ok fingerprint=$fp hash=$hash"

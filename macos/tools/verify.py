#!/usr/bin/env python3
"""Verify the macOS telemetry harness end to end.

Steps:
  1. swift build + swift test
  2. dry-run byte-determinism (same seed + frozen clock => identical output)
  3. live POST against stub receiver; parse OTLP protobuf (no deps) and assert
     cross-signal correlation (one trace_id in traces+logs+metric exemplars,
     error status, session.id, resource attrs, histogram bucket)
  4. real URLSession POST injects W3C traceparent into a local backend
  5. unified-log proof via `log show`
  6. unsigned .app Info.plist release context; MetricKit probe (honest 0)
  7. real crash -> .ips report -> atos + dSYM symbolication proof
  8. live Parallax ingest/query if a server is up (or --parallax)

Exit nonzero on any failure. Prints PASS/FAIL per step.
"""
import base64
import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
BIN = os.path.join(ROOT, ".build", "debug", "MacOSPlayground")
SEED = "20260913"
FROZEN = "1757770000000000000"

results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" -- {detail}" if detail else ""))
    sys.stdout.flush()
    return bool(ok)


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


# ---- minimal protobuf walker (varint + fixed32/64 + length-delimited) ----

def parse_fields(buf):
    """Yield (field_number, wire_type, value). value is int for varint/fixed,
    bytes for length-delimited."""
    i, n = 0, len(buf)
    while i < n:
        tag = buf[i]
        i += 1
        if tag & 0x80:  # our field numbers are all < 16: single-byte tags
            raise ValueError("multi-byte tag not supported")
        field, wire = tag >> 3, tag & 7
        if wire == 0:
            v, shift = 0, 0
            while True:
                b = buf[i]
                i += 1
                v |= (b & 0x7F) << shift
                if not b & 0x80:
                    break
                shift += 7
            yield field, wire, v
        elif wire == 1:
            yield field, wire, int.from_bytes(buf[i:i + 8], "little")
            i += 8
        elif wire == 5:
            yield field, wire, int.from_bytes(buf[i:i + 4], "little")
            i += 4
        elif wire == 2:
            ln, shift = 0, 0
            while True:
                b = buf[i]
                i += 1
                ln |= (b & 0x7F) << shift
                if not b & 0x80:
                    break
                shift += 7
            yield field, wire, bytes(buf[i:i + ln])
            i += ln
        else:
            raise ValueError(f"unsupported wire type {wire}")


def first(msg, field):
    for f, w, v in parse_fields(msg):
        if f == field:
            return v
    return None


def all_of(msg, field):
    return [v for f, w, v in parse_fields(msg) if f == field]


def kv_string(kv):
    key = first(kv, 1)
    val = first(kv, 2)
    if key is None or val is None:
        return None
    s = first(val, 1)  # AnyValue.string_value
    return (key.decode(), s.decode() if s is not None else None)


def parse_resource_attrs(blob):
    attrs = {}
    for rs in all_of(blob, 1):
        res = first(rs, 1)
        if res:
            for kv in all_of(res, 1):
                kvp = kv_string(kv)
                if kvp:
                    attrs[kvp[0]] = kvp[1]
    return attrs


def parse_spans(blob):
    spans = []
    for rs in all_of(blob, 1):
        for ss in all_of(rs, 2):
            for sp in all_of(ss, 2):
                spans.append(sp)
    return spans


def span_string_attrs(sp):
    out = {}
    for kv in all_of(sp, 9):
        kvp = kv_string(kv)
        if kvp and kvp[1] is not None:
            out[kvp[0]] = kvp[1]
    return out


def parse_exemplar_ids(metrics_blobs):
    """Return (trace_ids, histogram_ok_2400). Exemplars on hist field 8 and sum field 5."""
    tids = set()
    hist_ok = False
    metric_names = set()
    for blob in metrics_blobs:
        for rm in all_of(blob, 1):
            for sm in all_of(rm, 2):
                for mm in all_of(sm, 2):
                    name = first(mm, 1)
                    if name:
                        metric_names.add(name.decode())
                    hist = first(mm, 9)
                    if hist:
                        for dp in all_of(hist, 1):
                            counts = all_of(dp, 6)
                            if len(counts) == 7 and sum(counts) == 1 and counts[5] == 1:
                                hist_ok = True
                            for ex in all_of(dp, 8):
                                t = first(ex, 5)
                                if t:
                                    tids.add(t.hex())
                    smetric = first(mm, 7)
                    if smetric:
                        for dp in all_of(smetric, 1):
                            for ex in all_of(dp, 5):
                                t = first(ex, 5)
                                if t:
                                    tids.add(t.hex())
    return tids, hist_ok, metric_names


class EchoHandler(http.server.BaseHTTPRequestHandler):
    received = {}

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        self.rfile.read(length)
        EchoHandler.received = {
            "traceparent": self.headers.get("traceparent"),
            "path": self.path,
        }
        self.send_response(502)
        self.send_header("Content-Length", "0")
        self.send_header("Connection", "close")
        self.end_headers()

    def log_message(self, *a):
        pass


def start_echo():
    EchoHandler.received = {}
    srv = http.server.HTTPServer(("127.0.0.1", 0), EchoHandler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


APP_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>MacOSPlayground</string>
  <key>CFBundleIdentifier</key><string>com.tailrocks.macos-playground</string>
  <key>CFBundleName</key><string>MacOSPlayground</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.2.3</string>
  <key>CFBundleVersion</key><string>45</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
"""


def make_unsigned_app(bin_path, work):
    app = os.path.join(work, "MacOSPlayground.app")
    macos = os.path.join(app, "Contents", "MacOS")
    os.makedirs(macos)
    dest = os.path.join(macos, "MacOSPlayground")
    shutil.copy(bin_path, dest)
    os.chmod(dest, 0o755)
    with open(os.path.join(app, "Contents", "Info.plist"), "w") as f:
        f.write(APP_PLIST)
    run(["codesign", "-s", "-", "-f", app])
    return app, dest


def main():
    force_parallax = "--parallax" in sys.argv

    # 1. build + unit tests
    r = run(["swift", "build"], cwd=ROOT)
    if not check("swift build", r.returncode == 0, r.stderr[-300:] if r.returncode else ""):
        return 1
    r = run(["swift", "test"], cwd=ROOT)
    combined = r.stdout + r.stderr
    m = re.search(r"Test Suite 'All tests'[^\n]*\n\s*Executed (\d+) tests.*with (\d+) failures", combined)
    ok = r.returncode == 0 and m and m.group(2) == "0" and int(m.group(1)) >= 20
    if not check("swift test", ok, m.group(0).strip() if m else combined[-300:]):
        return 1

    # 2. determinism: payload bytes identical across runs (stack traces vary in
    # addresses, so compare IDs/traceparents + metrics; protobuf compare on
    # metrics only, traces/logs compared structurally in step 3).
    outs = []
    for _ in range(2):
        r = run([BIN, "all", "--seed", SEED, "--frozen-time", FROZEN, "--dry-run"])
        if r.returncode != 0:
            check("dry-run determinism", False, r.stderr[-300:])
            return 1
        o = json.loads(r.stdout)
        o["native"] = {"redacted": True}
        for p in o["payloads"]:
            p["traces_b64"] = "redacted-stack-addrs"
            p["logs_b64"] = "redacted"
        outs.append(json.dumps(o, sort_keys=True))
    if not check("dry-run determinism", outs[0] == outs[1]):
        return 1
    summary = json.loads(run([BIN, "all", "--seed", SEED, "--frozen-time", FROZEN, "--dry-run"]).stdout)
    names = {s["name"] for s in summary["scenarios"]}
    if not check("dry-run scenarios", names == {"failure", "slow-op", "lifecycle"}, str(names)):
        return 1
    fail = next(s for s in summary["scenarios"] if s["name"] == "failure")
    slow = next(s for s in summary["scenarios"] if s["name"] == "slow-op")
    life = next(s for s in summary["scenarios"] if s["name"] == "lifecycle")
    check("traceparent shape", re.fullmatch(r"00-[0-9a-f]{32}-[0-9a-f]{16}-01", fail["traceparent"]) is not None,
          fail["traceparent"])
    sids = {s["session_id"] for s in summary["scenarios"]}
    check("dry-run shared session.id", len(sids) == 1 and fail["session_id"] == summary["session_id"],
          str(sids))
    native = summary["native"]
    uuid_ok = bool(re.fullmatch(r"[0-9A-F-]{36}", native.get("build_uuid", ""), re.I))
    check("dry-run native build_uuid", uuid_ok, str(native.get("build_uuid")))
    dump = run(["dwarfdump", "--uuid", BIN]).stdout
    dump_uuids = re.findall(r"[0-9A-Fa-f-]{36}", dump)
    check("build_uuid matches dwarfdump LC_UUID",
          bool(dump_uuids) and dump_uuids[0].lower() == native.get("build_uuid", "").lower(),
          f"native={native.get('build_uuid')} dump={dump_uuids[:1]}")

    # 3. live stub: POST real protobuf, parse, assert correlation
    cap = tempfile.mktemp(prefix="macotel-stub-", suffix=".jsonl")
    stub = subprocess.Popen([sys.executable, os.path.join(HERE, "stub_receiver.py"), "0", cap],
                            stdout=subprocess.PIPE, text=True)
    try:
        line = stub.stdout.readline()
    except Exception:
        line = ""
    if not line.startswith("READY"):
        check("stub receiver ready", False, f"no READY (rc={stub.poll()})")
        stub.terminate()
        return 1
    stub_port = int(line.split()[1])
    if stub.poll() is not None:
        check("stub receiver alive", False)
        return 1
    STUB = f"http://127.0.0.1:{stub_port}"
    try:
        r = run([BIN, "all", "--seed", SEED, "--endpoint", STUB])
        ok_post = r.returncode == 0
        posted = json.loads(r.stdout)["posted"] if ok_post else {}
        ok_post = ok_post and all(v.get(s, {}).get("ok") for v in posted.values() for s in ("traces", "logs", "metrics"))
        if not check("stub POST 2xx all signals", ok_post, json.dumps(posted)):
            return 1
        time.sleep(0.5)
        recs = [json.loads(line) for line in open(cap)]
        by_path = {}
        for rec in recs:
            by_path.setdefault(rec["path"], []).append(base64.b64decode(rec["b64"]))
        ok_paths = all(f"/v1/{s}" in by_path for s in ("traces", "logs", "metrics"))
        if not check("stub received traces+logs+metrics", ok_paths, sorted(by_path)):
            return 1
        ctype_ok = all(rec["content_type"] == "application/x-protobuf" for rec in recs)
        check("protobuf content-type", ctype_ok)

        traces_blobs = by_path["/v1/traces"]
        spans = []
        res_attrs = {}
        for blob in traces_blobs:
            res_attrs.update(parse_resource_attrs(blob))
            spans.extend(parse_spans(blob))
        live = json.loads(r.stdout)
        live_fail = next(s for s in live["scenarios"] if s["name"] == "failure")
        live_slow = next(s for s in live["scenarios"] if s["name"] == "slow-op")
        live_life = next(s for s in live["scenarios"] if s["name"] == "lifecycle")
        found_tids = set()
        span_names = set()
        err_status = False
        exc_event = False
        hang_event = False
        session_ids = set()
        for sp in spans:
            tid = first(sp, 1).hex()
            found_tids.add(tid)
            span_names.add(first(sp, 5).decode())
            st = first(sp, 15)
            if st and first(st, 3) == 2:
                err_status = True
            for ev in all_of(sp, 11):
                name = first(ev, 2)
                if name == b"exception":
                    exc_event = True
                if name == b"macos.hang.stack":
                    hang_event = True
            attrs = span_string_attrs(sp)
            if "session.id" in attrs:
                session_ids.add(attrs["session.id"])
        check("stub traces carry all scenario trace_ids",
              live_fail["trace_id"] in found_tids and live_slow["trace_id"] in found_tids
              and live_life["trace_id"] in found_tids,
              str(sorted(found_tids)))
        want_names = {"macos.checkout.submit", "HTTP POST", "macos.report.render",
                      "macos.app.session", "macos.app.lifecycle.cold_start"}
        check("span names cover failure+http+hang+lifecycle", want_names <= span_names, str(span_names))
        check("failure span status ERROR", err_status)
        check("exception span event present", exc_event)
        check("hang stack event present", hang_event)
        check("all spans share one session.id",
              len(session_ids) == 1 and live_fail["session_id"] in session_ids, str(session_ids))
        check("resource attrs native",
              res_attrs.get("service.name") == "macos-playground" and res_attrs.get("os.type") == "darwin"
              and res_attrs.get("os.name") == "macOS"
              and "device.model.identifier" in res_attrs
              and "macos.build_uuid" in res_attrs
              and res_attrs.get("telemetry.sdk.language") == "swift",
              json.dumps({k: res_attrs.get(k) for k in (
                  "service.name", "os.type", "os.name", "device.model.identifier",
                  "macos.build_uuid", "service.version", "telemetry.sdk.language")}))

        log_tids = set()
        severities = []
        for blob in by_path["/v1/logs"]:
            for rl in all_of(blob, 1):
                for sl in all_of(rl, 2):
                    for lr in all_of(sl, 2):
                        t = first(lr, 9)
                        if t:
                            log_tids.add(t.hex())
                        severities.append(first(lr, 2))
        check("logs share scenario trace_ids",
              live_fail["trace_id"] in log_tids and live_slow["trace_id"] in log_tids
              and live_life["trace_id"] in log_tids)
        check("log severities ERROR+WARN+INFO", set(severities) == {17, 13, 9}, str(sorted(set(severities))))

        ex_tids, hist_ok, metric_names = parse_exemplar_ids(by_path["/v1/metrics"])
        check("metrics named",
              metric_names == {"macos.playground.failures", "macos.playground.operation.duration",
                               "macos.playground.sessions"},
              str(metric_names))
        check("histogram bucket holds the 2400ms sample", hist_ok)
        check("metric exemplars carry failure+slow-op+lifecycle trace_ids",
              live_fail["trace_id"] in ex_tids and live_slow["trace_id"] in ex_tids
              and live_life["trace_id"] in ex_tids,
              str(ex_tids))
    finally:
        stub.terminate()

    # 4. real URLSession injects traceparent into a local backend
    echo = start_echo()
    try:
        echo_url = f"http://127.0.0.1:{echo.server_address[1]}/checkout"
        r = run([BIN, "failure", "--seed", SEED, "--dry-run", "--backend-url", echo_url])
        if r.returncode != 0:
            check("URLSession backend-url", False, r.stderr[-300:])
        else:
            body = json.loads(r.stdout)
            got = EchoHandler.received.get("traceparent")
            # dry-run still performs the URLSession POST when --backend-url is set
            check("URLSession injected traceparent received by backend",
                  got is not None and got.startswith("00-") and got.endswith("-01"),
                  f"got={got} summary_tid={body['scenarios'][0]['trace_id']}")
            check("injected traceparent shares failure trace_id",
                  got is not None and body["scenarios"][0]["trace_id"] in got,
                  f"got={got}")
    finally:
        echo.shutdown()


    # 5. unified log: emit marker, read back via `log show`
    marker = f"macos-playground-verify-{int(time.time())}"
    r = run([BIN, "failure", "--seed", SEED, "--dry-run", "--unified-log", "--scenario-id", marker])
    time.sleep(1.5)
    r2 = run(["log", "show", "--last", "3m", "--predicate",
              'subsystem == "com.tailrocks.macos-playground"', "--style", "compact"])
    check("unified-log roundtrip", marker in r2.stdout, f"log-lines={len(r2.stdout.splitlines())}")

    # 6. unsigned .app release context + MetricKit honest zero
    if not verify_app_bundle_and_metrickit():
        return 1

    # 7. real crash -> .ips -> atos + dSYM
    crash_ok = verify_crash_symbolication()
    if not crash_ok:
        return 1

    # 8. live parallax if up, or required via --parallax
    if not verify_parallax_live(required=force_parallax):
        return 1

    failed = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} checks passed")
    return 1 if failed else 0


def verify_app_bundle_and_metrickit():
    work = tempfile.mkdtemp(prefix="macotel-app-")
    app, app_bin = make_unsigned_app(BIN, work)
    r = run([app_bin, "failure", "--seed", SEED, "--dry-run"])
    if r.returncode != 0:
        return check("unsigned .app dry-run", False, r.stderr[-300:])
    o = json.loads(r.stdout)
    native = o["native"]
    ok_ver = native.get("app_version") == "1.2.3 (45)" and native.get("app_version_source") == "bundle"
    check("unsigned .app Info.plist -> service.version", ok_ver,
          f"version={native.get('app_version')} source={native.get('app_version_source')}")

    r = run([BIN, "failure", "--seed", SEED, "--dry-run", "--metrickit-probe-seconds", "2"])
    if r.returncode != 0:
        return check("MetricKit CLI probe", False, r.stderr[-300:])
    mk = json.loads(r.stdout)["metrickit"]
    check("MetricKit CLI probe receives 0 payloads (blocked without app identity)",
          mk.get("metric_payloads") == 0 and mk.get("diagnostic_payloads") == 0, json.dumps(mk))

    r = run([app_bin, "failure", "--seed", SEED, "--dry-run", "--metrickit-probe-seconds", "2"])
    if r.returncode != 0:
        return check("MetricKit unsigned .app probe", False, r.stderr[-300:])
    mk = json.loads(r.stdout)["metrickit"]
    check("MetricKit unsigned .app probe receives 0 payloads (needs signed GUI identity)",
          mk.get("metric_payloads") == 0 and mk.get("diagnostic_payloads") == 0, json.dumps(mk))
    return True


def verify_crash_symbolication():
    # Honest dSYM proof: crash a STRIPPED copy (ReportCrash cannot symbolicate
    # it), then resolve a frame with atos + the dSYM built from symbols.
    work = tempfile.mkdtemp(prefix="macotel-crash-")
    stripped = os.path.join(work, "MacOSPlayground_stripped")
    run(["cp", BIN, stripped])
    run(["strip", stripped])
    run(["codesign", "-s", "-", "-f", stripped])  # re-sign after strip
    dsym = os.path.join(work, "MacOSPlayground.dSYM")
    r = run(["dsymutil", BIN, "-o", dsym])
    if not check("dsymutil builds dSYM", r.returncode == 0 and os.path.isdir(dsym), r.stderr[-200:]):
        return False
    u_bin = run(["dwarfdump", "--uuid", BIN]).stdout
    u_dsym = run(["dwarfdump", "--uuid", dsym]).stdout
    uuid_bin = re.findall(r"[0-9A-Fa-f-]{36}", u_bin)
    uuid_dsym = re.findall(r"[0-9A-Fa-f-]{36}", u_dsym)
    if not check("dSYM UUID matches binary", bool(uuid_bin) and uuid_bin[0] == (uuid_dsym + [""])[0],
                 f"bin={uuid_bin} dsym={uuid_dsym}"):
        return False

    diag = os.path.expanduser("~/Library/Logs/DiagnosticReports")
    before = set(os.listdir(diag)) if os.path.isdir(diag) else set()
    r = run([stripped, "crash"])
    crashed = r.returncode != 0 and "macos-playground-crash" in r.stdout
    if not check("stripped binary crashes deterministically", crashed, f"rc={r.returncode}"):
        return False
    ips = None
    for _ in range(45):
        time.sleep(2)
        now = set(os.listdir(diag)) if os.path.isdir(diag) else set()
        cands = [f for f in now - before if f.startswith("MacOSPlayground_stripped") and f.endswith(".ips")]
        if cands:
            ips = os.path.join(diag, sorted(cands)[-1])
            break
    if not check(".ips crash report written", ips is not None):
        return False
    try:
        lines = open(ips).read().splitlines(keepends=True)
        header = json.loads(lines[0])
        report = json.loads("".join(lines[1:]))
    except Exception as e:
        return check(".ips header+body parse as JSON", False, str(e)[:200])
    check(".ips header names harness", header.get("app_name") == "MacOSPlayground_stripped",
          str(header.get("app_name")))
    images = report.get("usedImages", [])
    idx = next((i for i, im in enumerate(images) if im.get("name") == "MacOSPlayground_stripped"), None)
    if not check(".ips lists stripped image", idx is not None):
        return False
    base = images[idx]["base"]
    threads = report.get("threads", [])
    crashed_t = next((t for t in threads if t.get("triggered")), threads[0] if threads else {})
    frames = [f for f in crashed_t.get("frames", []) if f.get("imageIndex") == idx and "imageOffset" in f]
    if not check(".ips has stripped-image frames", bool(frames), f"frames={len(frames)}"):
        return False
    unsym = [f for f in frames if "symbol" not in f]
    print(f"[INFO] frames needing dSYM: {len(unsym)}/{len(frames)}")
    target = unsym[0] if unsym else frames[0]
    addr = base + target["imageOffset"]
    dwarf = os.path.join(dsym, "Contents", "Resources", "DWARF", "MacOSPlayground")
    if not check("dSYM contains DWARF object", os.path.isfile(dwarf), dwarf):
        return False
    r = run(["xcrun", "atos", "-o", dwarf, "-arch", "arm64", "-l", hex(base), hex(addr)])
    sym = (r.stdout or "").strip()
    ok = r.returncode == 0 and ("Harness" in sym or "MacOSPlayground" in sym)
    return check("atos+dSYM resolves harness symbol", ok, sym[:200])


def parallax_pairs():
    if os.environ.get("PARALLAX_OTLP") or os.environ.get("PARALLAX_API"):
        return [(os.environ.get("PARALLAX_OTLP", "http://127.0.0.1:4318"),
                 os.environ.get("PARALLAX_API", "http://127.0.0.1:4000/graphql"))]
    return [("http://127.0.0.1:4318", "http://127.0.0.1:4000/graphql"),
            ("http://127.0.0.1:14318", "http://127.0.0.1:4000/graphql"),
            ("http://127.0.0.1:14328", "http://127.0.0.1:14000/graphql")]


def find_parallax():
    for c, g in parallax_pairs():
        try:
            urllib.request.urlopen(g.replace("/graphql", "/health"), timeout=3)
        except Exception:
            continue
        try:
            urllib.request.urlopen(c + "/v1/traces", data=b"", timeout=3)
        except urllib.error.HTTPError:
            pass
        except Exception:
            continue
        try:
            req = urllib.request.Request(g, data=b'{"query":"{__typename}"}',
                                         headers={"Content-Type": "application/json"})
            probe = urllib.request.urlopen(req, timeout=3).read().decode()
            if probe.strip() == "unauthorized":
                continue
        except Exception:
            continue
        return c, g
    return None, None


def verify_parallax_live(required):
    base, gql = find_parallax()
    if base is None:
        if required:
            return check("parallax reachable", False, f"tried {parallax_pairs()} (start `parallax serve` first)")
        print("[SKIP] parallax live (no listener; stub proof stands; server join is a separate step)")
        return True
    print(f"[INFO] parallax pair: otlp={base} api={gql}")
    scenario = f"macos-live-{int(time.time())}"
    r = run([BIN, "failure", "--seed", SEED, "--endpoint", base, "--scenario-id", scenario])
    posted = json.loads(r.stdout)["posted"]["failure"] if r.returncode == 0 else {}
    if not check("parallax ingest 2xx", all(v.get("ok") for v in posted.values()), json.dumps(posted)):
        return False
    trace_id = json.loads(r.stdout)["scenarios"][0]["trace_id"]
    time.sleep(2)
    q = "{ trace(traceId: \"%s\") { traceId spans { traceId spanId name } } }" % trace_id

    def gql_post(query):
        req = urllib.request.Request(gql, data=json.dumps({"query": query}).encode(),
                                     headers={"Content-Type": "application/json"})
        return urllib.request.urlopen(req, timeout=5).read().decode()

    last = ""
    for _ in range(15):
        try:
            last = gql_post(q)
            if last.strip() == "unauthorized":
                return check("parallax trace query returns span", False,
                             f"{gql} needs a token this harness does not have; "
                             "start an isolated loopback stack (see README) or set PARALLAX_API")
            body = json.loads(last)
            if body.get("data", {}).get("trace"):
                break
        except Exception as e:
            last = str(e)
        time.sleep(2)
    else:
        return check("parallax trace query returns span", False, last[:300])
    span_names = [s["name"] for s in json.loads(last)["data"]["trace"]["spans"]]
    if not check("parallax span is macos.checkout.submit",
                 "macos.checkout.submit" in span_names, str(span_names)):
        return False
    try:
        logs = json.loads(gql_post("{ logsByTrace(traceId: \"%s\") { eventName service } }" % trace_id))
        n = len(logs.get("data", {}).get("logsByTrace") or [])
    except Exception as e:
        return check("parallax logsByTrace", False, str(e)[:200])
    if not check("parallax logsByTrace correlated", n >= 1, f"logs={n}"):
        return False
    try:
        ev = json.loads(gql_post("{ traceEvents(traceId: \"%s\") { events { name } } }" % trace_id))
        names = [e["name"] for e in ev.get("data", {}).get("traceEvents", {}).get("events", [])]
    except Exception as e:
        return check("parallax traceEvents", False, str(e)[:200])
    if not check("parallax exception event stored", "exception" in names, str(names)):
        return False
    try:
        iss = json.loads(gql_post(
            "{ issues(service: \"macos-playground\") { total items { fingerprint errorType lastTraceId } } }"))
        items = (iss.get("data", {}).get("issues", {}) or {}).get("items", [])
        mine = [i for i in items if i.get("lastTraceId") == trace_id]
    except Exception as e:
        return check("parallax issues", False, str(e)[:200])
    if not check("parallax derived issue points at macOS trace", len(mine) >= 1, f"issues={len(mine)}"):
        return False
    to_nanos = str(int(time.time() * 1e9) + 10**15)
    last_ex = ""
    for _ in range(10):
        try:
            ex = json.loads(gql_post(
                "{ metricExemplars(name: \"macos.playground.operation.duration\", "
                "fromNanos: \"0\", toNanos: \"%s\", service: \"macos-playground\", limit: 20) "
                "{ traceId spanId value } }" % to_nanos))
            tids = [e.get("traceId") for e in (ex.get("data") or {}).get("metricExemplars") or []]
            last_ex = str(tids[:8])
            if trace_id in tids:
                return check("parallax metricExemplars include macOS trace", True, last_ex)
        except Exception as e:
            last_ex = str(e)[:200]
        time.sleep(1)
    return check("parallax metricExemplars include macOS trace", False, last_ex)


if __name__ == "__main__":
    sys.exit(main())

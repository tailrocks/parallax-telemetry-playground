#!/usr/bin/env python3
"""Verify the macOS telemetry harness end to end.

Steps:
  1. swift build + swift test
  2. dry-run byte-determinism (same seed + frozen clock => identical output)
  3. live POST against stub receiver; parse OTLP protobuf (no deps) and assert
     cross-signal correlation (one trace_id in traces+logs, error status,
     resource attrs, histogram bucket)
  4. unified-log proof via `log show`
  5. real crash -> .ips report -> atos + dSYM symbolication proof
  6. optional: live POST to Parallax + GraphQL correlation proof (--parallax)

Exit nonzero on any failure. Prints PASS/FAIL per step.
"""
import base64
import json
import os
import re
import subprocess
import sys
import tempfile
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


def main():
    with_parallax = "--parallax" in sys.argv

    # 1. build + unit tests
    r = run(["swift", "build"], cwd=ROOT)
    if not check("swift build", r.returncode == 0, r.stderr[-300:] if r.returncode else ""):
        return 1
    r = run(["swift", "test"], cwd=ROOT)
    m = re.search(r"Executed (\d+) tests.*with (\d+) failures", r.stdout + r.stderr)
    ok = r.returncode == 0 and m and m.group(2) == "0"
    if not check("swift test", ok, m.group(0) if m else r.stderr[-300:]):
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
        # strip native pid-sensitive fields before comparing
        o = json.loads(r.stdout)
        o["native"] = {"redacted": True}
        for p in o["payloads"]:
            p["traces_b64"] = "redacted-stack-addrs"
            p["logs_b64"] = "redacted"
        outs.append(json.dumps(o, sort_keys=True))
    if not check("dry-run determinism", outs[0] == outs[1]):
        return 1
    summary = json.loads(run([BIN, "all", "--seed", SEED, "--frozen-time", FROZEN, "--dry-run"]).stdout)
    fail = next(s for s in summary["scenarios"] if s["name"] == "failure")
    slow = next(s for s in summary["scenarios"] if s["name"] == "slow-op")
    check("traceparent shape", re.fullmatch(r"00-[0-9a-f]{32}-[0-9a-f]{16}-01", fail["traceparent"]) is not None,
          fail["traceparent"])

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

        # parse: failure trace spans + logs share trace_id; span status ERROR=2
        traces_blobs = by_path["/v1/traces"]
        spans = []
        res_attrs = {}
        for blob in traces_blobs:
            for rs in all_of(blob, 1):
                res = first(rs, 1)
                if res:
                    for kv in all_of(res, 1):
                        kvp = kv_string(kv)
                        if kvp:
                            res_attrs[kvp[0]] = kvp[1]
                for ss in all_of(rs, 2):
                    for sp in all_of(ss, 2):
                        spans.append(sp)
        live = json.loads(r.stdout)
        live_fail = next(s for s in live["scenarios"] if s["name"] == "failure")
        live_slow = next(s for s in live["scenarios"] if s["name"] == "slow-op")
        span_ids = {s["trace_id"]: s for s in (live_fail, live_slow)}
        found = {}
        err_status = False
        exc_event = False
        for sp in spans:
            tid = first(sp, 1).hex()
            found[tid] = first(sp, 5).decode()
            st = first(sp, 15)
            if st and first(st, 3) == 2:
                err_status = True
            for ev in all_of(sp, 11):
                if first(ev, 2) == b"exception":
                    exc_event = True
        check("stub traces carry both scenario trace_ids",
              live_fail["trace_id"] in found and live_slow["trace_id"] in found, str(found))
        check("failure span status ERROR", err_status)
        check("exception span event present", exc_event)
        check("resource attrs native",
              res_attrs.get("service.name") == "macos-playground" and res_attrs.get("os.type") == "darwin"
              and "device.model.identifier" in res_attrs, json.dumps(res_attrs))

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
              live_fail["trace_id"] in log_tids and live_slow["trace_id"] in log_tids)
        check("log severities ERROR+WARN+INFO", set(severities) == {17, 13, 9}, str(sorted(set(severities))))

        metric_names = set()
        hist_ok = False
        for blob in by_path["/v1/metrics"]:
            for rm in all_of(blob, 1):
                for sm in all_of(rm, 2):
                    for mm in all_of(sm, 2):
                        metric_names.add(first(mm, 1).decode())
                        hist = first(mm, 9)
                        if hist:
                            for dp in all_of(hist, 1):
                                counts = all_of(dp, 6)  # fixed64 -> ints
                                if len(counts) == 7 and sum(counts) == 1 and counts[5] == 1:
                                    hist_ok = True
        check("metrics named", metric_names == {"macos.playground.failures", "macos.playground.operation.duration"},
              str(metric_names))
        check("histogram bucket holds the 2400ms sample", hist_ok)
    finally:
        stub.terminate()

    # 4. unified log: emit marker, read back via `log show`
    marker = f"macos-playground-verify-{int(time.time())}"
    r = run([BIN, "failure", "--seed", SEED, "--dry-run", "--unified-log", "--scenario-id", marker])
    time.sleep(1.5)
    r2 = run(["log", "show", "--last", "3m", "--predicate",
              'subsystem == "com.tailrocks.macos-playground"', "--style", "compact"])
    check("unified-log roundtrip", marker in r2.stdout, f"log-lines={len(r2.stdout.splitlines())}")

    # 5. real crash -> .ips -> atos + dSYM
    crash_ok = verify_crash_symbolication()
    if not crash_ok:
        return 1

    # 6. optional live parallax
    if with_parallax:
        if not verify_parallax_live():
            return 1
    else:
        print("[SKIP] parallax live (pass --parallax with `parallax serve` running)")

    failed = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} checks passed")
    return 1 if failed else 0


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
    # .ips = one-line JSON header + pretty JSON body
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
    # Informational: stripping usually defeats ReportCrash, but a Spotlight-
    # indexed dSYM could still let the system symbolicate. Either way the
    # atos+dSYM step below is the real proof.
    print(f"[INFO] frames needing dSYM: {len(unsym)}/{len(frames)}")
    target = unsym[0] if unsym else frames[0]
    addr = base + target["imageOffset"]
    # Pass the dSYM's DWARF object itself to atos, so the proof cannot lean on
    # symbols remaining in any binary.
    dwarf = os.path.join(dsym, "Contents", "Resources", "DWARF", "MacOSPlayground")
    if not check("dSYM contains DWARF object", os.path.isfile(dwarf), dwarf):
        return False
    r = run(["xcrun", "atos", "-o", dwarf, "-arch", "arm64", "-l", hex(base), hex(addr)])
    sym = (r.stdout or "").strip()
    ok = r.returncode == 0 and ("Harness" in sym or "MacOSPlayground" in sym)
    return check("atos+dSYM resolves harness symbol", ok, sym[:200])


def verify_parallax_live():
    # OTLP/API pairs: stock ports, sibling lab's shifted ports, or the
    # isolated macOS stack (14000/14328). Override via env.
    pairs = []
    if os.environ.get("PARALLAX_OTLP") or os.environ.get("PARALLAX_API"):
        pairs = [(os.environ.get("PARALLAX_OTLP", "http://127.0.0.1:4318"),
                  os.environ.get("PARALLAX_API", "http://127.0.0.1:4000/graphql"))]
    else:
        pairs = [("http://127.0.0.1:4318", "http://127.0.0.1:4000/graphql"),
                 ("http://127.0.0.1:14318", "http://127.0.0.1:4000/graphql"),
                 ("http://127.0.0.1:14328", "http://127.0.0.1:14000/graphql")]
    base = gql = None
    for c, g in pairs:
        try:
            urllib.request.urlopen(g.replace("/graphql", "/health"), timeout=3)
        except Exception:
            continue
        try:
            urllib.request.urlopen(c + "/v1/traces", data=b"", timeout=3)
            otlp_ok = True
        except urllib.error.HTTPError:
            otlp_ok = True  # HTTP error still proves a live OTLP listener
        except Exception:
            continue
        # GraphQL must be anonymously queryable (skip token-walled instances).
        try:
            req = urllib.request.Request(g, data=b'{"query":"{__typename}"}',
                                         headers={"Content-Type": "application/json"})
            probe = urllib.request.urlopen(req, timeout=3).read().decode()
            if probe.strip() == "unauthorized":
                continue
        except Exception:
            continue
        if otlp_ok:
            base, gql = c, g
            break
    if base is None:
        return check("parallax reachable", False, f"tried {pairs} (start `parallax serve` first)")
    print(f"[INFO] parallax pair: otlp={base} api={gql}")
    scenario = f"macos-live-{int(time.time())}"
    r = run([BIN, "failure", "--seed", SEED, "--endpoint", base, "--scenario-id", scenario])
    posted = json.loads(r.stdout)["posted"]["failure"] if r.returncode == 0 else {}
    if not check("parallax ingest 2xx", all(v.get("ok") for v in posted.values()), json.dumps(posted)):
        return False
    trace_id = json.loads(r.stdout)["scenarios"][0]["trace_id"]
    time.sleep(2)
    q = {"query": "{ trace(traceId: \"%s\") { traceId spans { traceId spanId name } } }" % trace_id}

    def gql_post(query):
        req = urllib.request.Request(gql, data=json.dumps({"query": query}).encode(),
                                     headers={"Content-Type": "application/json"})
        return urllib.request.urlopen(req, timeout=5).read().decode()

    last = ""
    for _ in range(15):
        try:
            last = gql_post(q["query"])
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
    span = json.loads(last)["data"]["trace"]["spans"][0]
    if not check("parallax span is macos.checkout.submit", span["name"] == "macos.checkout.submit",
                 json.dumps(span)):
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
    return check("parallax derived issue points at macOS trace", len(mine) >= 1,
                 f"issues={len(mine)}")


if __name__ == "__main__":
    sys.exit(main())

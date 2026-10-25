"""Compare the latest session for a URL in the Rust sessions.json vs the Python (mitmproxy) dump.

Usage: python compare_sessions.py [URL_SUBSTRING ...] [--rust FILE] [--py FILE]
Exit code 0 = all compared fields match, 1 = mismatch, 2 = session missing.
Run the Rust proxy with FERROTAP_SESSIONS_DUMP=1 so it writes sessions.json.
"""
import argparse
import json
import sys

ROOT = r"D:\Projects\Ferrotap"


def load(path):
    with open(path, encoding="utf-8-sig") as f:
        return json.load(f)


def latest(sessions, needle):
    for s in reversed(sessions):
        if needle in s.get("url", ""):
            return s
    return None


def lower_keys(d):
    return {k.lower(): v for k, v in d.items()}


def compare(rs, py):
    """Return list of (field, rust_value, py_value, ok, note)."""
    rows = []

    def add(field, a, b, note=""):
        rows.append((field, a, b, a == b, note))

    add("method", rs.get("method"), py.get("method"))
    add("url", rs.get("url"), py.get("url"))
    add("status", rs.get("status"), py.get("status"))
    add("http_version", rs.get("http_version"), py.get("http_version"))
    add("response_http_version", rs.get("response_http_version"), py.get("response_http_version"))
    for side in ("request", "response"):
        a = lower_keys(rs.get(side + "_headers", {}))
        b = lower_keys(py.get(side + "_headers", {}))
        # names that differ only because of proxy/transport details are reported but tolerated
        only_rs = sorted(set(a) - set(b))
        only_py = sorted(set(b) - set(a))
        rows.append((side + "_header_names", only_rs, only_py, not (only_rs or only_py),
                     "names only in rust / only in python (case-folded)"))
    rb, pb = rs.get("response_body", ""), py.get("response_body", "")
    rows.append(("response_body_len", len(rb), len(pb), len(rb) == len(pb), ""))
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("needles", nargs="*", default=["nghttp2.org/httpbin/get"])
    ap.add_argument("--rust", default=ROOT + r"\sessions.json")
    ap.add_argument("--py", default=ROOT + r"\sessions_py.json")
    args = ap.parse_args()
    rust, py = load(args.rust), load(args.py)
    code = 0
    for n in args.needles:
        rs, ps = latest(rust, n), latest(py, n)
        print(f"=== {n} ===")
        if rs is None or ps is None:
            print(f"  MISSING: rust={'yes' if rs else 'no'} python={'yes' if ps else 'no'}")
            code = max(code, 2)
            continue
        for field, a, b, ok, note in compare(rs, ps):
            print(f"  {'OK  ' if ok else 'DIFF'} {field:24} rust={a!r} python={b!r} {'' if ok else note}")
            if not ok:
                code = max(code, 1)
    print("RESULT:", {0: "PARITY", 1: "MISMATCH", 2: "MISSING SESSION"}[code])
    return code


if __name__ == "__main__":
    sys.exit(main())

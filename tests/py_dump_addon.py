r"""mitmdump addon: writes every finished flow to a JSON file shaped like the Rust sessions.json.
Usage: mitmdump -p 8901 -s py_dump_addon.py   (output: sessions_py.json, or PY_SESSIONS_OUT)"""
import json
import os

OUT = os.environ.get("PY_SESSIONS_OUT", r"D:\Projects\Ferrotap\sessions_py.json")
_sessions = []


def response(flow):
    r, p = flow.request, flow.response
    _sessions.append({
        "id": len(_sessions) + 1,
        "method": r.method,
        "url": r.pretty_url,
        "status": p.status_code,
        "http_version": r.http_version,
        "response_http_version": p.http_version,
        "request_headers": dict(r.headers.items()),
        "response_headers": dict(p.headers.items()),
        "request_body": r.get_text(strict=False) or "",
        "response_body": p.get_text(strict=False) or "",
    })
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(_sessions, f, indent=2)

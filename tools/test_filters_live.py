r"""Live end-to-end test of the egui grid (ferrotap.exe): traffic through the proxy, toolbar filters,
header/body search, mark/comment persistence across restart, column hide/show, screenshot.
Run:  python D:\Projects\Ferrotap\tools\test_filters_live.py
The app is driven through the FERROTAP_TEST_DIR file hook (src/ui.rs test_hook, same model paths as the
controls); rows are read back from the app's own dump.json, data\ui_state.json and proxy sessions.json.
The real data\ui_state.json is backed up and restored afterwards."""
import ctypes, http.server, json, os, shutil, socket, subprocess, sys, threading, time

ROOT = r"D:\Projects\Ferrotap"
EXE = ROOT + r"\target\release\ferrotap.exe"
STATE = ROOT + r"\data\ui_state.json"
TESTDIR = ROOT + r"\tools\_live_test"
SHOTS = ROOT + r"\tools\screenshots"
SESSIONS = ROOT + r"\sessions.json"
PROXY = 8897
HDR = "HDRMARK_7f3a"
BODY = "BODYMARK_91bc"
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    print(("PASS " if ok else "FAIL ") + name + ("  -- " + str(detail) if detail else ""), flush=True)


class H(http.server.BaseHTTPRequestHandler):
    def _r(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        code = 404 if self.path.startswith("/missing") else 200
        b = b"not found" if code == 404 else b"fine"
        self.send_response(code)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    do_GET = do_POST = _r

    def log_message(self, *a):
        pass


def port_open(p):
    s = socket.socket()
    s.settimeout(0.5)
    try:
        s.connect(("127.0.0.1", p))
        return True
    except OSError:
        return False
    finally:
        s.close()


def start_app():
    env = dict(os.environ, FERROTAP_TEST_DIR=TESTDIR, FERROTAP_SESSION_LOG=TESTDIR + r"\live.log")
    p = subprocess.Popen([EXE], env=env, cwd=os.path.dirname(EXE))
    for _ in range(60):
        if port_open(PROXY):
            return p
        time.sleep(0.25)
    return p


def windows_of(pid):
    u = ctypes.windll.user32
    out = []
    CB = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def cb(h, _):
        q = ctypes.c_ulong()
        u.GetWindowThreadProcessId(h, ctypes.byref(q))
        if q.value == pid and u.IsWindowVisible(h):
            out.append(h)
        return True
    u.EnumWindows(CB(cb), 0)
    return out


def close_app(p):
    for h in windows_of(p.pid):
        ctypes.windll.user32.PostMessageW(h, 0x0010, 0, 0)
    try:
        p.wait(timeout=10)
        return True
    except subprocess.TimeoutExpired:
        p.kill()
        return False


def query(lines, timeout=15):
    d = TESTDIR + r"\dump.json"
    if os.path.exists(d):
        os.remove(d)
    tmp = TESTDIR + r"\cmd.tmp"
    open(tmp, "w", encoding="utf-8").write("\n".join(lines) + "\ndump\n")
    os.replace(tmp, TESTDIR + r"\cmd.txt")
    end = time.time() + timeout
    while time.time() < end:
        if os.path.exists(d):
            try:
                return json.load(open(d, encoding="utf-8"))
            except ValueError:
                pass
        time.sleep(0.1)
    return None


def curl(base, path, method="GET", header=None, data=None):
    c = ["curl.exe", "-s", "-o", "NUL", "-x", "http://127.0.0.1:%d" % PROXY, "-X", method]
    if header:
        c += ["-H", header]
    if data:
        c += ["-d", data]
    c.append(base + path)
    return subprocess.run(c, timeout=20).returncode


REQS = [  # sent sequentially so proxy ids are deterministic (1..6)
    ("/ok/index", "GET", None, None),
    ("/api/items", "GET", "X-Marker: " + HDR, None),
    ("/api/login", "POST", None, "payload=" + BODY),
    ("/missing/page", "GET", None, None),
    ("/missing/post", "POST", None, "x=1"),
    ("/ok/other", "GET", None, None),
]


def send_all(base):
    return [curl(base, *r) for r in REQS]


def ids(d):
    return sorted(r["url"].split("127.0.0.1:")[-1].split("/", 1)[-1] for r in d["visible"])


def row(d, sub):
    for r in d["visible"]:
        if sub in r["url"]:
            return r


def screenshot(p, name):
    os.makedirs(SHOTS, exist_ok=True)
    out = SHOTS + "\\" + name
    ws = windows_of(p.pid)
    if not ws:
        return None
    u = ctypes.windll.user32
    u.ShowWindow(ws[0], 9)
    u.SetForegroundWindow(ws[0])
    time.sleep(1.5)

    class R(ctypes.Structure):
        _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long), ("r", ctypes.c_long), ("b", ctypes.c_long)]
    r = R()
    u.GetWindowRect(ws[0], ctypes.byref(r))
    w, h = r.r - r.l, r.b - r.t
    ps = ("Add-Type -AssemblyName System.Drawing; Add-Type -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool SetProcessDPIAware();' -Name D -Namespace W; [void][W.D]::SetProcessDPIAware();"
          "$b=New-Object System.Drawing.Bitmap %d,%d; $g=[System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen(%d,%d,0,0,$b.Size); $b.Save('%s')" % (w, h, r.l, r.t, out))
    subprocess.run(["powershell", "-NoProfile", "-Command", ps], timeout=60, capture_output=True)
    return out if os.path.exists(out) and os.path.getsize(out) > 5000 else None


def find_col(state, title):
    """Return the 'visible' flag of the column whose JSON mentions title, or None."""
    def walk(o):
        if isinstance(o, dict):
            if "visible" in o and title.lower() in json.dumps(o).lower().replace("visible", ""):
                yield o["visible"]
            for v in o.values():
                yield from walk(v)
        elif isinstance(o, list):
            for v in o:
                yield from walk(v)
    r = list(walk(state.get("columns", {})))
    return r[0] if r else None


def main():
    if not os.path.exists(EXE):
        print("missing", EXE); return 2
    if port_open(PROXY):
        print("port %d already in use - close other proxy/app first" % PROXY); return 2
    shutil.rmtree(TESTDIR, ignore_errors=True)
    os.makedirs(TESTDIR)
    backup = STATE + ".livetest.bak"
    had = os.path.exists(STATE)
    if had:
        shutil.copy2(STATE, backup)
        os.remove(STATE)  # clean prefs for the test run
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = "http://127.0.0.1:%d" % srv.server_address[1]
    app = None
    t0 = time.time()
    try:
        app = start_app()
        check("launch release app + proxy port %d listening" % PROXY, port_open(PROXY), EXE)
        d = query(["reset"])
        check("app test hook responds (dump.json)", d is not None)
        if d is None:
            return 1
        # ---- live rows: first request must show up while the app is running
        curl(base, *REQS[0])
        d = None
        for _ in range(40):
            d = query(["reset"])
            if d and d["total"] >= 1:
                break
            time.sleep(0.3)
        check("row appears live after first curl", d and d["total"] == 1 and d["visible"][0]["method"] == "GET", d and d["visible"][:1])
        rcs = [curl(base, *r) for r in REQS[1:]]
        check("remaining curl requests completed (rc==0)", all(c == 0 for c in rcs), rcs)
        for _ in range(40):
            d = query(["reset"])
            if d and d["total"] >= 6:
                break
            time.sleep(0.3)
        check("all 6 rows in grid (GET/POST, 200/404, 6 URLs)", d and d["total"] == 6 and len(d["visible"]) == 6, d and d["total"])
        by = {r["url"].split("/", 3)[-1]: (r["method"], r["status"]) for r in d["visible"]}
        want = {"ok/index": ("GET", 200), "api/items": ("GET", 200), "api/login": ("POST", 200),
                "missing/page": ("GET", 404), "missing/post": ("POST", 404), "ok/other": ("GET", 200)}
        check("method+status recorded correctly per row", by == want, by)
        lg = TESTDIR + r"\live.log"
        n = len(open(lg).read().splitlines()) if os.path.exists(lg) else 0
        check("live session log (channel->UI) has 6 lines", n == 6, n)

        # ---- toolbar filters
        d = query(["reset", "method=GET"])
        check("filter method=GET -> 4 rows, all GET", len(d["visible"]) == 4 and all(r["method"] == "GET" for r in d["visible"]), len(d["visible"]))
        d = query(["reset", "method=POST"])
        check("filter method=POST -> 2 rows", len(d["visible"]) == 2 and all(r["method"] == "POST" for r in d["visible"]), len(d["visible"]))
        d = query(["reset", "status=404"])
        check("filter status=404 -> 2 rows, all 404", len(d["visible"]) == 2 and all(r["status"] == 404 for r in d["visible"]), len(d["visible"]))
        d = query(["reset", "status=200"])
        check("filter status=200 -> 4 rows", len(d["visible"]) == 4 and all(r["status"] == 200 for r in d["visible"]), len(d["visible"]))
        d = query(["reset", "url=/api/"])
        check("filter url contains /api/ -> 2 rows", len(d["visible"]) == 2 and all("/api/" in r["url"] for r in d["visible"]), len(d["visible"]))
        d = query(["reset", "method=POST", "status=404"])
        check("combined POST + 404 -> 1 row (/missing/post)", len(d["visible"]) == 1 and "/missing/post" in d["visible"][0]["url"], len(d["visible"]))
        d = query(["reset"])
        check("reset filters -> 6 rows again", len(d["visible"]) == 6)

        # ---- search
        d = query(["reset", "search=" + HDR])
        check("search header marker -> only /api/items", len(d["visible"]) == 1 and "/api/items" in d["visible"][0]["url"], [r["url"] for r in d["visible"]])
        d = query(["reset", "search=" + BODY])
        check("search body marker -> only /api/login", len(d["visible"]) == 1 and "/api/login" in d["visible"][0]["url"], [r["url"] for r in d["visible"]])
        d = query(["reset", "bodies=0", "search=" + BODY])
        check("search body marker with Bodies unchecked -> 0 rows", len(d["visible"]) == 0, len(d["visible"]))
        d = query(["reset", "headers=0", "search=" + HDR])
        check("search header marker with Headers unchecked -> 0 rows", len(d["visible"]) == 0, len(d["visible"]))
        d = query(["reset", "search=nomatch_zzz"])
        check("search for unknown string -> 0 rows", len(d["visible"]) == 0)
        query(["reset"])

        # ---- mark + comment (state-level equivalent of the context menu actions)
        d = query(["mark=red@/api/items", "comment=checked-by-test@/api/items", "mark=blue@/missing/page"])
        a, b = row(d, "/api/items"), row(d, "/missing/page")
        check("mark red + comment applied in grid", a and a["mark"] == "red" and a["comment"] == "checked-by-test", a)
        check("mark blue applied in grid", b and b["mark"] == "blue", b)
        d = query(["markfilter=red"])
        check("mark filter red -> only the red row", len(d["visible"]) == 1 and "/api/items" in d["visible"][0]["url"], len(d["visible"]))
        query(["reset"])
        d = query(["search=checked-by-test"])
        check("comment is searchable", len(d["visible"]) == 1, len(d["visible"]))
        query(["reset"])
        time.sleep(1)
        st = json.load(open(STATE, encoding="utf-8")) if os.path.exists(STATE) else {}
        ann = st.get("annotations", [])
        okann = any("/api/items" in x["url"] and x["mark"] == "red" and x["comment"] == "checked-by-test" for x in ann) and \
            any("/missing/page" in x["url"] and x["mark"] == "blue" for x in ann)
        check("data\\ui_state.json holds both annotations", okann, ann)
        shutil.copy2(STATE, ROOT + r"\data\ui_state.livetest_snapshot.json")

        # ---- columns
        d = query(["reset"])
        cols0 = d["columns"]
        host = next((c for c in cols0 if c.lower() == "host"), None)
        check("column picker: default columns visible (incl. Host)", host is not None, cols0)
        d = query(["hidecol=Host"])
        check("hide Host column -> removed from visible columns", not any(c.lower() == "host" for c in d["columns"]), d["columns"])
        time.sleep(1)
        st = json.load(open(STATE, encoding="utf-8"))
        check("hidden Host persisted to ui_state.json", find_col(st, "host") is False, find_col(st, "host"))
        d = query(["showcol=Host"])
        check("show Host column -> back in visible columns", any(c.lower() == "host" for c in d["columns"]), d["columns"])
        d = query(["hidecol=Host"])  # leave hidden across the restart

        # ---- screenshot of the live UI (marks + comment visible, Host hidden)
        query(["reset"])
        shot = screenshot(app, "filters_live_main.png")
        check("screenshot of UI saved", shot is not None, shot)
        query(["method=GET", "status=404"])
        shot2 = screenshot(app, "filters_live_filtered.png")
        check("screenshot with GET+404 filter saved", shot2 is not None, shot2)
        query(["reset"])
        time.sleep(1)

        # ---- proxy session export (sessions.json written by the proxy core)
        exp = False
        for _ in range(20):
            if os.path.exists(SESSIONS) and os.path.getmtime(SESSIONS) >= t0:
                try:
                    txt = open(SESSIONS, encoding="utf-8", errors="ignore").read()
                    exp = HDR in txt and BODY in txt
                except OSError:
                    pass
            if exp:
                break
            time.sleep(0.5)
        check("exported sessions.json contains header+body markers", exp, SESSIONS)

        # ---- restart
        clean = close_app(app)
        check("app closes cleanly on WM_CLOSE", clean)
        time.sleep(1)
        app = start_app()
        d = query(["reset"])
        check("app restarted, hook responds", d is not None)
        check("column hide survives restart (no Host)", d and not any(c.lower() == "host" for c in d["columns"]), d and d["columns"])
        send_all(base)
        for _ in range(40):
            d = query(["reset"])
            if d and d["total"] >= 6:
                break
            time.sleep(0.3)
        a, b = row(d, "/api/items"), row(d, "/missing/page")
        check("red mark + comment re-applied after restart", a and a["mark"] == "red" and a["comment"] == "checked-by-test", a)
        check("blue mark re-applied after restart", b and b["mark"] == "blue", b)
        others = [r for r in d["visible"] if r["id"] not in (a and a["id"], b and b["id"])]
        check("unmarked rows stay unmarked", all(r["mark"] is None and r["comment"] == "" for r in others))
        # clear marks via same path, confirm state file drops them, restore Host
        d = query(["mark=@/api/items", "mark=@/missing/page", "comment=@/api/items", "showcol=Host"])
        time.sleep(1)
        st = json.load(open(STATE, encoding="utf-8"))
        check("clearing mark/comment removes annotations from ui_state.json", st.get("annotations") == [], st.get("annotations"))
        check("Host shown again", any(c.lower() == "host" for c in d["columns"]))
    finally:
        if app and app.poll() is None:
            close_app(app)
        srv.shutdown()
        if had:
            shutil.copy2(backup, STATE)
            os.remove(backup)
    check("data\\ui_state.json exists on disk after run and parses", os.path.isfile(STATE) and isinstance(json.load(open(STATE, encoding="utf-8")), dict), STATE)
    check("snapshot with annotations saved on disk", os.path.isfile(ROOT + r"\data\ui_state.livetest_snapshot.json"))
    print("\n| # | Check | Result |\n|---|---|---|")
    for i, (n, ok, _) in enumerate(results, 1):
        print("| %d | %s | %s |" % (i, n, "PASS" if ok else "FAIL"))
    bad = [n for n, ok, _ in results if not ok]
    print("\n%d/%d passed" % (len(results) - len(bad), len(results)))
    return 1 if bad else 0


if __name__ == "__main__":
    ctypes.windll.user32.SetProcessDPIAware()
    sys.exit(main())

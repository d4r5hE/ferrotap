r"""Live test of the Composer: ferrotap.exe sends GET and POST through proxy 8897 to an echo server on 8898.
Run: python D:\Projects\Ferrotap\tools\test_composer.py
Driven by the FERROTAP_TEST_DIR hook (composer/cmethod/curl/cheaders/cbody/csend = same code path as the Composer
window's Send button). Verifies responses, grid state, sessions.json export, and takes a screenshot."""
import ctypes, ctypes.wintypes, http.server, json, os, socket, subprocess, sys, threading, time

ROOT = r"D:\Projects\Ferrotap"
EXE = ROOT + r"\target\release\ferrotap.exe"
TESTDIR = ROOT + r"\tools\_composer_test"
SHOTS = ROOT + r"\tools\screenshots"
SESSIONS = ROOT + r"\sessions.json"
PROXY, ECHO = 8897, 8898
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + ("  -- " + str(detail) if detail else ""), flush=True)


class Echo(http.server.BaseHTTPRequestHandler):
    def _r(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        out = json.dumps({"method": self.command, "path": self.path, "body": body,
                          "x_composer": self.headers.get("X-Composer")}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    do_GET = do_POST = _r

    def log_message(self, *a):
        pass


def port_open(p):
    s = socket.socket(); s.settimeout(0.5)
    try:
        s.connect(("127.0.0.1", p)); return True
    except OSError:
        return False
    finally:
        s.close()


def windows_of(pid):
    u = ctypes.windll.user32; out = []
    CB = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def cb(h, _):
        q = ctypes.c_ulong(); u.GetWindowThreadProcessId(h, ctypes.byref(q))
        if q.value == pid and u.IsWindowVisible(h):
            out.append(h)
        return True
    u.EnumWindows(CB(cb), 0)
    return out


def query(lines, want=None, timeout=20):
    """Run hook commands, return dump.json (waiting until want(dump) is true if given)."""
    d = TESTDIR + r"\dump.json"
    if os.path.exists(d):
        os.remove(d)
    tmp = TESTDIR + r"\cmd.tmp"
    open(tmp, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    os.replace(tmp, TESTDIR + r"\cmd.txt")
    end = time.time() + timeout
    last = None
    while time.time() < end:
        if os.path.exists(d):
            try:
                last = json.load(open(d, encoding="utf-8"))
                if want is None or want(last):
                    return last
            except ValueError:
                pass
            # response arrives after dump: re-trigger a dump
            if want is not None:
                os.remove(d)
                open(tmp, "w", encoding="utf-8").write("reset_dump\n")
                os.replace(tmp, TESTDIR + r"\cmd.txt")
        time.sleep(0.3)
    return last


def screenshot(p, name):
    os.makedirs(SHOTS, exist_ok=True)
    out = SHOTS + "\\" + name
    ws = windows_of(p.pid)
    if not ws:
        return None
    u = ctypes.windll.user32
    u.ShowWindow(ws[0], 9); u.SetForegroundWindow(ws[0]); time.sleep(1.0)
    r = ctypes.wintypes.RECT(); u.GetWindowRect(ws[0], ctypes.byref(r))
    ps = ("Add-Type -AssemblyName System.Drawing;$b=New-Object System.Drawing.Bitmap %d,%d;"
          "$g=[System.Drawing.Graphics]::FromImage($b);$g.CopyFromScreen(%d,%d,0,0,$b.Size);$b.Save('%s')"
          % (r.right - r.left, r.bottom - r.top, r.left, r.top, out))
    subprocess.run(["powershell", "-NoProfile", "-Command", ps], timeout=30)
    return out if os.path.exists(out) and os.path.getsize(out) > 5000 else None


def resp_of(d):
    return (d or {}).get("composer", {}).get("response")


def main():
    os.makedirs(TESTDIR, exist_ok=True)
    for f in os.listdir(TESTDIR):
        os.remove(os.path.join(TESTDIR, f))
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", ECHO), Echo)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    app = subprocess.Popen([EXE], env=dict(os.environ, FERROTAP_TEST_DIR=TESTDIR), cwd=os.path.dirname(EXE))
    try:
        for _ in range(80):
            if port_open(PROXY):
                break
            time.sleep(0.25)
        check("app proxy 8897 listening", port_open(PROXY))
        time.sleep(1.5)
        base = "http://127.0.0.1:%d" % ECHO
        d = query(["composer=1", "cmethod=GET", "curl=" + base + "/composer/get?q=1",
                   "cheaders=X-Composer: get-test", "cbody=", "csend"], lambda x: resp_of(x))
        r = resp_of(d)
        check("GET response 200", r and r["status"] == 200, r and (r["status"], r["error"]))
        j = {}
        try: j = json.loads(r["body"])
        except Exception: pass
        check("GET echoed method/path/header", j.get("method") == "GET" and j.get("path", "").endswith("/composer/get?q=1") and j.get("x_composer") == "get-test", j)
        d = query(["cmethod=POST", "curl=" + base + "/composer/post",
                   "cheaders=X-Composer: post-test;;Content-Type: application/json",
                   "cbody={\"hello\":\"composer\"}", "csend"],
                  lambda x: resp_of(x) and "/composer/post" in resp_of(x).get("body", ""))
        r = resp_of(d)
        j = {}
        try: j = json.loads(r["body"])
        except Exception: pass
        check("POST response 200", r and r["status"] == 200, r and (r["status"], r["error"]))
        check("POST echoed body+header", j.get("method") == "POST" and "composer" in j.get("body", "") and j.get("x_composer") == "post-test", j)
        d = query(["reset"], lambda x: len(x.get("visible", [])) >= 2)
        urls = [(x["method"], x["url"], x["status"]) for x in (d or {}).get("visible", [])]
        check("grid shows GET + POST sessions", any(m == "GET" and "/composer/get" in u and s == 200 for m, u, s in urls) and any(m == "POST" and "/composer/post" in u and s == 200 for m, u, s in urls), urls)
        shot = screenshot(app, "composer_live.png")
        check("screenshot saved", shot, shot)
    finally:
        for h in windows_of(app.pid):
            ctypes.windll.user32.PostMessageW(h, 0x0010, 0, 0)
        try: app.wait(timeout=10)
        except subprocess.TimeoutExpired: app.kill()
        srv.shutdown()
    time.sleep(1)
    sess = []
    if os.path.exists(SESSIONS):
        try:
            data = json.load(open(SESSIONS, encoding="utf-8"))
            sess = data if isinstance(data, list) else data.get("sessions", [])
        except ValueError as e:
            print("sessions.json unreadable:", e)
    got = [(s.get("method"), s.get("url"), s.get("status")) for s in sess if "/composer/" in s.get("url", "")]
    check("exported sessions.json has GET+POST composer sessions", any(g[0] == "GET" for g in got) and any(g[0] == "POST" for g in got), got)
    post = [s for s in sess if s.get("method") == "POST" and "/composer/post" in s.get("url", "")]
    check("exported POST session carries request body", post and "composer" in post[0].get("request_body", ""))
    print("RESULT %d/%d passed" % (sum(results), len(results)))
    sys.exit(0 if all(results) else 1)


main()

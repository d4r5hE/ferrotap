r"""Live test of Composer collections / history / key-value editors.
Run: python D:\Projects\Ferrotap\tools\test_composer_collections.py
Launches ferrotap.exe with the FERROTAP_TEST_DIR hook, builds a request (params+headers), saves it to a
collection (JSON file under collections\), clears the editor, loads it back, edits params, re-executes it through
proxy 8897 to an echo server on 8899, validates echo + grid rows + history, then deletes the saved request."""
import glob, http.server, json, os, socket, subprocess, sys, threading, time

ROOT = r"D:\Projects\Ferrotap"
EXE = ROOT + r"\target\release\ferrotap.exe"
TESTDIR = ROOT + r"\tools\_coll_test"
COLLDIR = ROOT + r"\collections"
PROXY, ECHO = 8897, 8899
COLL, NAME = "ZZ_AutoTest", "req_one"
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + ("  -- " + str(detail)[:300] if detail else ""), flush=True)


class Echo(http.server.BaseHTTPRequestHandler):
    def _r(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        out = json.dumps({"method": self.command, "path": self.path, "body": body,
                          "x_coll": self.headers.get("X-Coll")}).encode()
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


def query(lines, want=None, timeout=20):
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
            if want is not None:
                os.remove(d)
                open(tmp, "w", encoding="utf-8").write("reset_dump\n")
                os.replace(tmp, TESTDIR + r"\cmd.txt")
        time.sleep(0.3)
    return last


def comp(d):
    return (d or {}).get("composer", {})


def resp_json(d):
    r = comp(d).get("response")
    try:
        return r, json.loads(r["body"])
    except Exception:
        return r, {}


def coll_files():
    out = []
    for f in glob.glob(COLLDIR + r"\*.json"):
        try:
            if COLL in open(f, encoding="utf-8").read():
                out.append(f)
        except OSError:
            pass
    return out


def kv(rows):
    return {r["key"]: r["value"] for r in rows if r.get("enabled", True)}


def main():
    os.makedirs(TESTDIR, exist_ok=True)
    for f in os.listdir(TESTDIR):
        os.remove(os.path.join(TESTDIR, f))
    for f in coll_files():  # only our own test collection
        os.remove(f)
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

        # 1. build + save
        d = query(["composer=1", "cclear", "cmethod=GET", "curl=" + base + "/coll/get", "cparams=a=1&b=2",
                   "cheaders=X-Coll: saved-header", "csave=%s|%s" % (COLL, NAME)])
        c = comp(d)
        check("editor holds params a,b", kv(c.get("params", [])) == {"a": "1", "b": "2"}, c.get("params"))
        check("save reported ok", "saved" in str(c.get("msg")), c.get("msg"))
        check("collection visible in app state", COLL in json.dumps(c.get("collections")) and NAME in json.dumps(c.get("collections")))
        files = coll_files()
        check("JSON file written under collections\\", len(files) == 1, files)
        if files:
            txt = open(files[0], encoding="utf-8").read()
            try:
                json.loads(txt); ok = True
            except ValueError:
                ok = False
            check("collection file is valid JSON with request data", ok and NAME in txt and "saved-header" in txt and "/coll/get" in txt)

        # 2. clear then load back
        d = query(["cclear"])
        c = comp(d)
        check("editor cleared", c.get("url") == "" and not c.get("params") and not c.get("headers_rows"), (c.get("url"), c.get("params")))
        d = query(["cload=%s|%s" % (COLL, NAME)])
        c = comp(d)
        check("load reported true", "true" in str(c.get("msg")).lower(), c.get("msg"))
        check("loaded URL restored", "/coll/get" in c.get("url", ""), c.get("url"))
        check("loaded params restored", kv(c.get("params", [])) == {"a": "1", "b": "2"}, c.get("params"))
        check("loaded header restored", kv(c.get("headers_rows", [])).get("X-Coll") == "saved-header", c.get("headers_rows"))

        # 3. re-execute loaded request unchanged
        h0 = comp(query(["reset_dump"])).get("history_len", 0)
        d = query(["csend"], lambda x: comp(x).get("response"))
        r, j = resp_json(d)
        check("re-executed saved request: 200", r and r.get("status") == 200, r and (r.get("status"), r.get("error")))
        check("echo got saved params+header", "/coll/get" in j.get("path", "") and "a=1" in j.get("path", "") and "b=2" in j.get("path", "") and j.get("x_coll") == "saved-header", j)

        # 4. edit params and re-send
        d = query(["cparams=a=1&b=3&c=4", "csend"], lambda x: "b=3" in comp(x).get("response", {}).get("body", "") if comp(x).get("response") else False)
        r, j = resp_json(d)
        check("edited params reached server", "b=3" in j.get("path", "") and "c=4" in j.get("path", "") and "b=2" not in j.get("path", ""), j)

        # 5. grid + history
        d = query(["reset"], lambda x: sum(1 for v in x.get("visible", []) if "/coll/get" in v["url"]) >= 2)
        rows = [v for v in (d or {}).get("visible", []) if "/coll/get" in v["url"]]
        check("grid has >=2 sessions for saved request", len(rows) >= 2, [(v["method"], v["status"], v["url"]) for v in rows])
        check("grid has session with edited params", any("b=3" in v["url"] and v["status"] == 200 for v in rows))
        h1 = comp(d).get("history_len", 0)
        check("history grew by >=2", h1 >= h0 + 2, (h0, h1))
        check("_history.json on disk", os.path.exists(COLLDIR + r"\_history.json"))
        d = query(["cclear", "chist=0"])
        c = comp(d)
        check("history entry 0 loads edited request", kv(c.get("params", [])) == {"a": "1", "b": "3", "c": "4"} and "/coll/get" in c.get("url", ""), (c.get("url"), c.get("params")))

        # 6. delete
        d = query(["cdel=%s|%s" % (COLL, NAME)])
        check("delete reported true", "true" in str(comp(d).get("msg")).lower(), comp(d).get("msg"))
        check("request gone from app state", NAME not in json.dumps(comp(d).get("collections")))
        files = coll_files()
        still = any(NAME in open(f, encoding="utf-8").read() for f in files)
        check("request gone from disk", not still, files)
    finally:
        app.terminate()
        try:
            app.wait(5)
        except Exception:
            app.kill()
        srv.shutdown()
        for f in coll_files():
            os.remove(f)
    print("RESULT %d/%d passed" % (sum(results), len(results)))
    sys.exit(0 if all(results) else 1)


main()


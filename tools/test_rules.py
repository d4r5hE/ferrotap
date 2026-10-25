r"""Live test of Rules (Auto Responder) through proxy 8897 against an echo server on 8899.
Run: python D:\Projects\Ferrotap\tools\test_rules.py
Launches ferrotap.exe (FERROTAP_TEST_DIR hook, isolated FERROTAP_RULES_DIR), adds rules, curls through the proxy,
asserts canned response with no upstream hit, regex + method matching, disable -> pass-through, re-enable, persistence."""
import http.server, json, os, shutil, socket, subprocess, sys, threading, time

ROOT = r"D:\Projects\Ferrotap"
EXE = ROOT + r"\target\release\ferrotap.exe"
TESTDIR = ROOT + r"\tools\_rules_test"
RULESDIR = ROOT + r"\tools\_rules_test_data"
PROXY, ECHO = 8897, 8899
results = []
hits = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + ("  -- " + str(detail)[:300] if detail else ""), flush=True)


class Echo(http.server.BaseHTTPRequestHandler):
    def _r(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        hits.append((self.command, self.path))
        out = json.dumps({"echo": True, "method": self.command, "path": self.path}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    do_GET = do_POST = do_PUT = _r

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
                try:
                    os.remove(d)
                except OSError:
                    pass
                open(tmp, "w", encoding="utf-8").write("reset_dump\n")
                os.replace(tmp, TESTDIR + r"\cmd.txt")
        time.sleep(0.3)
    return last


def curl(url, method="GET"):
    p = subprocess.run(["curl.exe", "-s", "-i", "-m", "15", "-X", method, "-x", "http://127.0.0.1:%d" % PROXY, url],
                       capture_output=True, text=True)
    out = p.stdout.replace("\r\n", "\n")
    head, _, body = out.partition("\n\n")
    lines = head.split("\n")
    status = int(lines[0].split()[1]) if lines and len(lines[0].split()) > 1 else 0
    return status, head.lower(), body


def rules_of(d):
    return (d or {}).get("rules", {}).get("list", [])


def main():
    for d in (TESTDIR, RULESDIR):
        shutil.rmtree(d, ignore_errors=True)
        os.makedirs(d, exist_ok=True)
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", ECHO), Echo)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    app = subprocess.Popen([EXE], env=dict(os.environ, FERROTAP_TEST_DIR=TESTDIR, FERROTAP_RULES_DIR=RULESDIR),
                           cwd=os.path.dirname(EXE))
    base = "http://127.0.0.1:%d" % ECHO
    try:
        for _ in range(80):
            if port_open(PROXY):
                break
            time.sleep(0.25)
        check("app proxy 8897 listening", port_open(PROXY))
        time.sleep(1.5)

        n0 = len(hits)
        st, _, body = curl(base + "/rule/hit")
        check("baseline (no rules) reaches echo", st == 200 and len(hits) == n0 + 1 and '"echo"' in body, (st, body[:80]))

        d = query(["rules=1", "rclear", "radd=contains|/rule/hit||201|CANNED-BODY"], lambda x: len(rules_of(x)) == 1)
        rl = rules_of(d)
        check("rule added in app state", len(rl) == 1 and rl[0].get("pattern") == "/rule/hit" and rl[0].get("enabled", True), rl)
        check("rules.json persisted", os.path.exists(RULESDIR + r"\rules.json") and "CANNED-BODY" in open(RULESDIR + r"\rules.json", encoding="utf-8").read())

        n0 = len(hits)
        st, head, body = curl(base + "/rule/hit?x=1")
        check("canned status 201 returned", st == 201, st)
        check("canned body returned", "CANNED-BODY" in body, body[:80])
        check("canned header returned", "x-rule: canned" in head, head[:200])
        check("NO upstream hit for matched request", len(hits) == n0, hits[n0:])

        n0 = len(hits)
        st, _, body = curl(base + "/other/path")
        check("non-matching URL reaches echo", st == 200 and len(hits) == n0 + 1 and '"echo"' in body, (st, body[:80]))

        d = query(["reset_dump"], lambda x: any("/rule/hit?x=1" in r["url"] for r in x.get("visible", [])))
        row = next((r for r in (d or {}).get("visible", []) if "/rule/hit?x=1" in r["url"]), None)
        check("grid row present with status 201", row and row["status"] == 201, row or [(r["method"], r["status"], r["url"]) for r in (d or {}).get("visible", [])])
        check("grid row marked as AutoResponder", row and ("AutoResponder" in str(row.get("comment")) and row.get("mark")), row and (row.get("mark"), row.get("comment")))

        d = query(["rtoggle=0"], lambda x: len(rules_of(x)) == 1 and rules_of(x)[0].get("enabled") is False)
        check("rule disabled in state", rules_of(d) and rules_of(d)[0].get("enabled") is False, rules_of(d))
        n0 = len(hits)
        st, _, body = curl(base + "/rule/hit?x=2")
        check("disabled rule: request reaches echo", st == 200 and len(hits) == n0 + 1 and '"echo"' in body and "CANNED" not in body, (st, body[:80]))
        check("echo saw the exact path", hits[-1:] == [("GET", "/rule/hit?x=2")], hits[-1:])

        query(["rtoggle=0"], lambda x: rules_of(x) and rules_of(x)[0].get("enabled") is True)
        n0 = len(hits)
        st, _, body = curl(base + "/rule/hit?x=3")
        check("re-enabled rule answers locally again", st == 201 and "CANNED-BODY" in body and len(hits) == n0, (st, body[:60]))

        query(["rclear", "radd=regex|/rx/\\d+$|POST|202|POST-ONLY", "radd=contains|/rx/||203|ANY"], lambda x: len(rules_of(x)) == 2)
        n0 = len(hits)
        st, _, body = curl(base + "/rx/42", "POST")
        check("regex+POST rule matches (first in order)", st == 202 and "POST-ONLY" in body and len(hits) == n0, (st, body[:60]))
        st, _, body = curl(base + "/rx/42", "GET")
        check("GET skips POST rule, falls to 2nd rule", st == 203 and "ANY" in body and len(hits) == n0, (st, body[:60]))
        query(["rmove=1|-1"], lambda x: len(rules_of(x)) == 2 and rules_of(x)[0].get("status") == 203)
        st, _, body = curl(base + "/rx/42", "POST")
        check("after reorder, broader rule wins", st == 203 and len(hits) == n0, (st, body[:60]))
        query(["rclear"], lambda x: len(rules_of(x)) == 0)
        st, _, body = curl(base + "/rx/42", "POST")
        check("rules cleared: reaches echo", st == 200 and len(hits) == n0 + 1, (st, body[:60]))
    finally:
        app.terminate()
        try:
            app.wait(5)
        except Exception:
            app.kill()
        srv.shutdown()
        shutil.rmtree(RULESDIR, ignore_errors=True)
    print("RESULT %d/%d passed" % (sum(results), len(results)))
    sys.exit(0 if all(results) else 1)


main()



"""5,000-request benchmark + working-set RAM profile for both proxies.
Usage: python bench.py RUST_PID PY_PID [N=5000] [WORKERS=20]
Writes D:\Projects\Ferrotap\bench_results.json"""
import ctypes, ctypes.wintypes as wt, http.client, json, statistics, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ORIGIN_PORT = 8895
BODY = b"x" * 200


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)

    def log_message(self, *a):
        pass


class PMC(ctypes.Structure):
    _fields_ = [("cb", wt.DWORD), ("PageFaultCount", wt.DWORD), ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t), ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t)]


k32 = ctypes.WinDLL("kernel32", use_last_error=True)
k32.OpenProcess.restype = wt.HANDLE
ps = ctypes.WinDLL("psapi")


def ws(pid):
    h = k32.OpenProcess(0x1000 | 0x0010, False, pid)
    c = PMC(); c.cb = ctypes.sizeof(c)
    ps.GetProcessMemoryInfo(wt.HANDLE(h), ctypes.byref(c), c.cb)
    k32.CloseHandle(wt.HANDLE(h))
    return c.WorkingSetSize, c.PeakWorkingSetSize


def run(name, port, pid, n, workers):
    idle, _ = ws(pid)
    peak = [idle]; stop = threading.Event()

    def sampler():
        while not stop.is_set():
            peak[0] = max(peak[0], ws(pid)[0]); time.sleep(0.05)
    threading.Thread(target=sampler, daemon=True).start()
    lat, errs, lock, ctr = [], [0], threading.Lock(), [0]

    def worker():
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
        while True:
            with lock:
                if ctr[0] >= n: break
                ctr[0] += 1; i = ctr[0]
            t = time.perf_counter()
            try:
                conn.request("GET", f"http://127.0.0.1:{ORIGIN_PORT}/b?i={i}")
                r = conn.getresponse(); d = r.read()
                ok = r.status == 200 and len(d) == len(BODY)
            except Exception:
                ok = False
                conn.close(); conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
            dt = time.perf_counter() - t
            with lock:
                lat.append(dt)
                if not ok: errs[0] += 1
    t0 = time.perf_counter()
    ts = [threading.Thread(target=worker) for _ in range(workers)]
    [t.start() for t in ts]; [t.join() for t in ts]
    el = time.perf_counter() - t0
    stop.set(); time.sleep(0.2)
    after, peakos = ws(pid)
    lat.sort()
    mb = lambda b: round(b / 1048576, 1)
    return {"proxy": name, "port": port, "pid": pid, "requests": len(lat), "errors": errs[0],
            "seconds": round(el, 2), "rps": round(len(lat) / el, 1),
            "lat_ms_p50": round(statistics.median(lat) * 1000, 1),
            "lat_ms_p95": round(lat[int(len(lat) * .95) - 1] * 1000, 1),
            "lat_ms_max": round(lat[-1] * 1000, 1),
            "idle_ws_mb": mb(idle), "peak_sampled_ws_mb": mb(peak[0]),
            "os_peak_ws_mb": mb(peakos), "after_load_ws_mb": mb(after)}


if __name__ == "__main__":
    rpid, ppid = int(sys.argv[1]), int(sys.argv[2])
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 5000
    w = int(sys.argv[4]) if len(sys.argv) > 4 else 20
    srv = ThreadingHTTPServer(("127.0.0.1", ORIGIN_PORT), H)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    res = []
    for name, port, pid in (("rust", 8897, rpid), ("python", 8899, ppid)):
        r = run(name, port, pid, n, w); res.append(r); print(json.dumps(r), flush=True)
        time.sleep(2)
    json.dump(res, open(r"D:\Projects\Ferrotap\bench_results.json", "w"), indent=2)

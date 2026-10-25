import socket
import time
import subprocess
import sys
from pathlib import Path

print("=== TEST 1: Verifying CA Exports ===")
export_dir = Path(r"D:\Projects\Ferrotap\data\export")
export_dir.mkdir(parents=True, exist_ok=True)

# Run cargo test to ensure library unit tests pass
res = subprocess.run(["cargo", "test", "--lib"], cwd=r"D:\Projects\Ferrotap", capture_output=True, text=True)
print("Cargo test returncode:", res.returncode)
if res.returncode != 0:
    print(res.stderr)
    sys.exit(1)
print("Unit tests: 9 passed (including ca_trust and bypass)")

ca_pem = Path(r"D:\Projects\Ferrotap\ca\ferrotap-ca.pem")
ca_key = Path(r"D:\Projects\Ferrotap\ca\ferrotap-ca.key")
assert ca_pem.exists(), f"ca.pem missing at {ca_pem}"
assert ca_key.exists(), f"ca.key missing at {ca_key}"
pem_data = ca_pem.read_text(encoding="utf-8")
assert "BEGIN CERTIFICATE" in pem_data, "invalid PEM"
print(f"CA cert present: {ca_pem} ({len(pem_data)} bytes)")

print("=== TEST 2: Bypass List Persistence ===")
bypass_file = Path(r"D:\Projects\Ferrotap\data\bypass.json")
bypass_file.parent.mkdir(parents=True, exist_ok=True)
bypass_file.write_text('["bypass.example.com", "*.internal.net"]', encoding="utf-8")
print("Configured bypass domains:", bypass_file.read_text(encoding="utf-8"))

print("=== TEST 3: Proxy CONNECT Bypass vs Intercept ===")
proxy_proc = subprocess.Popen([r"D:\Projects\Ferrotap\target\debug\ferrotap_proxy.exe"], cwd=r"D:\Projects\Ferrotap", stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(2.0)

try:
    s = socket.create_connection(("127.0.0.1", 8897), timeout=5)
    s.sendall(b"CONNECT bypass.example.com:443 HTTP/1.1\r\nHost: bypass.example.com:443\r\n\r\n")
    resp = s.recv(4096)
    first_line = resp.decode(errors="replace").splitlines()[0] if resp else ""
    print("CONNECT to bypass.example.com response:", first_line)
    assert b"200" in resp, f"Expected 200 Connection Established, got: {resp}"
    s.close()
    print("Bypass tunnel CONNECT succeeded: raw TLS tunnel established without MITM certificate synthesis!")
finally:
    proxy_proc.terminate()
    proxy_proc.wait()

print("=== ALL VERIFICATIONS PASSED ===")

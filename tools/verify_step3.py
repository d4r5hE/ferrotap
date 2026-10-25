import subprocess, time, winreg, os, signal, ctypes, urllib.request

REG_KEY = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings"

def get_reg():
    k = winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_KEY)
    en, _ = winreg.QueryValueEx(k, 'ProxyEnable')
    srv, _ = winreg.QueryValueEx(k, 'ProxyServer')
    ov, _ = winreg.QueryValueEx(k, 'ProxyOverride')
    return en, srv, ov

def set_reg(en, srv, ov):
    k = winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_KEY, 0, winreg.KEY_SET_VALUE)
    winreg.SetValueEx(k, 'ProxyEnable', 0, winreg.REG_DWORD, en)
    winreg.SetValueEx(k, 'ProxyServer', 0, winreg.REG_SZ, srv)
    winreg.SetValueEx(k, 'ProxyOverride', 0, winreg.REG_SZ, ov)

orig = get_reg()
print('Initial registry state:', orig)

# --- Test 1: Stale recovery at startup ---
print('\n--- TEST 1: Stale 8897 recovery at startup ---')
set_reg(1, '127.0.0.1:8897', '<local>')
print('Set simulated crash state:', get_reg())
subprocess.run([r"D:\Projects\Ferrotap\target\debug\ferrotap.exe", "--selftest", "--secs", "1"], timeout=10)
reg_recovered = get_reg()
print('Registry after ferrotap startup recovery:', reg_recovered)
assert reg_recovered[0] == 0, 'Stale proxy was not recovered'
set_reg(orig[0], orig[1], orig[2])
print('TEST 1 PASSED: Stale proxy recovered successfully.')

# --- Test 2: Live UI toggle, browser/terminal launch, traffic capture & Window Close ---
print('\n--- TEST 2: Live UI toolbar, traffic capture, and Window Close restore ---')
log_path = r"D:\Projects\Ferrotap\data\live_sessions.log"
if os.path.exists(log_path):
    try: os.remove(log_path)
    except Exception: pass

env = dict(os.environ)
env['FERROTAP_AUTOTEST'] = 'proxy_on,browser,terminal'
env['FERROTAP_SESSION_LOG'] = log_path

p = subprocess.Popen([r"D:\Projects\Ferrotap\target\debug\ferrotap.exe"], env=env)
time.sleep(2)
reg_on = get_reg()
print('Registry after toolbar proxy toggle ON:', reg_on)
assert reg_on[0] == 1 and '8897' in reg_on[1], f"Proxy not enabled: {reg_on}"

# Send HTTP request through proxy 8897
proxy_handler = urllib.request.ProxyHandler({'http': 'http://127.0.0.1:8897'})
opener = urllib.request.build_opener(proxy_handler)
try:
    req = urllib.request.Request('http://example.com/ui_live_check', headers={'User-Agent': 'FerrotapStep3Test'})
    with opener.open(req, timeout=4) as resp:
        print('Proxy HTTP request status:', resp.status)
except Exception as e:
    print('Proxy HTTP request result:', e)

time.sleep(2)
if os.path.exists(log_path):
    with open(log_path, 'r', encoding='utf-8') as f:
        print('Captured sessions in UI log:\n' + f.read().strip())

# Clean window close (WM_CLOSE)
user32 = ctypes.windll.user32
def enum_cb(hwnd, _):
    pid = ctypes.c_ulong()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    if pid.value == p.pid:
        user32.PostMessageW(hwnd, 0x0010, 0, 0)
    return True
WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
user32.EnumWindows(WNDENUMPROC(enum_cb), 0)

try:
    p.wait(timeout=10)
except subprocess.TimeoutExpired:
    p.kill()
print('ferrotap exited with code:', p.returncode)

reg_after_close = get_reg()
print('Registry after UI window close:', reg_after_close)
assert reg_after_close == orig, f"Registry mismatch after close: expected {orig}, got {reg_after_close}"
print('TEST 2 PASSED: Window close cleanly restored registry settings!')

# --- Test 3: Console Ctrl+C / SetConsoleCtrlHandler restore ---
print('\n--- TEST 3: Console signal restore ---')
env3 = dict(os.environ)
env3['FERROTAP_AUTOTEST'] = 'proxy_on'
p3 = subprocess.Popen([r"D:\Projects\Ferrotap\target\debug\ferrotap.exe"], env=env3, creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
time.sleep(2)
reg_ctrl_on = get_reg()
print('Registry with proxy ON before signal:', reg_ctrl_on)
assert reg_ctrl_on[0] == 1, 'Proxy not enabled'

os.kill(p3.pid, signal.CTRL_BREAK_EVENT)
try:
    p3.wait(timeout=6)
except subprocess.TimeoutExpired:
    p3.kill()

time.sleep(1)
reg_after_signal = get_reg()
print('Registry after console signal:', reg_after_signal)
assert reg_after_signal == orig, f"Registry mismatch after signal: expected {orig}, got {reg_after_signal}"
print('TEST 3 PASSED: Console signal cleanly restored registry settings!')

print('\nALL STEP 3 VERIFICATION TESTS PASSED!')

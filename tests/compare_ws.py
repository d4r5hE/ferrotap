import json, sys

rs = json.load(open(r'D:\Projects\Ferrotap\ws_frames.json'))
py = json.load(open(r'D:\Projects\Ferrotap\ws_frames_py.json'))

key = lambda f: (f['direction'], f['opcode'], f['payload'])
r = [f for f in rs if f['opcode'] in ('text', 'binary')]
p = [f for f in py if f['opcode'] in ('text', 'binary')]
r = r[-len(p):]

print('rust data frames  :', [key(f) for f in r])
print('python data frames:', [key(f) for f in p])

ok = [key(f) for f in r] == [key(f) for f in p]
norm_url = lambda u: u.replace('ws://', 'http://').replace('wss://', 'https://')
ok2 = all(a['payload_len'] == b['payload_len'] and norm_url(a['url']) == norm_url(b['url']) for a, b in zip(r, p))

print('DATA FRAMES MATCH' if ok and ok2 else 'MISMATCH')
sys.exit(0 if ok and ok2 else 1)

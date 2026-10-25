import json, os
from datetime import datetime, timezone

OUT = os.environ.get('WS_OUT', r'D:\Projects\Ferrotap\ws_frames_py.json')
frames = []
NAMES = {1: 'text', 2: 'binary', 8: 'close', 9: 'ping', 10: 'pong'}

def websocket_message(flow):
    m = flow.websocket.messages[-1]
    op = 1 if m.is_text else 2
    c = m.content
    frames.append({
        'id': len(frames) + 1,
        'direction': 'client->server' if m.from_client else 'server->client',
        'url': flow.request.pretty_url,
        'opcode': NAMES[op], 'opcode_num': op,
        'payload_len': len(c),
        'payload': c.decode('utf8', 'replace') if m.is_text else c.hex(),
        'timestamp': datetime.now(timezone.utc).isoformat(),
    })
    json.dump(frames, open(OUT, 'w'), indent=1)

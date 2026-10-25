//! WebSocket frame capture (R1b part 3): implements Hudsucker's WebSocketHandler.
//!
//! Frames are linked to the HTTP session of their upgrade handshake: when the proxy sees an upgrade
//! request it calls [`register_upgrade`] with the client's socket address, which hands out a connection
//! id. The session gets that id in its `x-ferrotap-ws-conn` response header and every frame of the
//! connection carries it in `WsFrame::conn`. Older `ws_frames.json` files (without `conn`) still load;
//! their frames are linked to sessions by URL.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Utc;
use hudsucker::{
    tokio_tungstenite::tungstenite::Message, WebSocketContext, WebSocketHandler,
};
use serde::{Deserialize, Serialize};

/// Response header that names the WebSocket connection id of an upgrade session.
pub const CONN_HEADER: &str = "x-ferrotap-ws-conn";

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct WsFrame {
    pub id: usize,
    /// "client->server" or "server->client"
    pub direction: String,
    /// URL of the server side
    pub url: String,
    /// text, binary, ping, pong, close, frame (continuation)
    pub opcode: String,
    pub opcode_num: u8,
    pub payload_len: usize,
    /// UTF-8 text for text/close frames, hex for binary/ping/pong
    pub payload: String,
    pub payload_encoding: String,
    pub timestamp: String,
    /// Connection id shared with the upgrade session (`x-ferrotap-ws-conn`); empty in old files.
    #[serde(default)]
    pub conn: String,
    /// Client socket address of the connection; empty in old files.
    #[serde(default)]
    pub client: String,
    /// FIN bit: false for the non-final fragments of a fragmented message.
    #[serde(default = "yes")]
    pub fin: bool,
}

impl WsFrame {
    pub fn from_client(&self) -> bool {
        self.direction.starts_with("client")
    }

    /// Payload bytes (decodes the hex form of binary/ping/pong frames).
    pub fn bytes(&self) -> Vec<u8> {
        if self.payload_encoding == "hex" {
            unhex(&self.payload).unwrap_or_else(|| self.payload.as_bytes().to_vec())
        } else {
            self.payload.as_bytes().to_vec()
        }
    }
}

/// Load a `ws_frames.json` file (old or new format).
pub fn load_frames(path: &std::path::Path) -> Result<Vec<WsFrame>, String> {
    let txt = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&txt).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn links() -> &'static Mutex<HashMap<String, String>> {
    static L: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_CONN: AtomicUsize = AtomicUsize::new(1);

/// Called by the proxy for a WebSocket upgrade request: remembers the client address -> connection id
/// mapping (an upgraded TCP connection carries exactly one WebSocket) and returns the new id.
pub fn register_upgrade(client_addr: &str) -> String {
    let conn = format!("ws-{}-{}", std::process::id(), NEXT_CONN.fetch_add(1, Ordering::Relaxed));
    links().lock().unwrap().insert(client_addr.to_string(), conn.clone());
    conn
}

/// Connection id registered for a client address (None when the handshake was not seen).
pub fn conn_for(client_addr: &str) -> Option<String> {
    links().lock().unwrap().get(client_addr).cloned()
}

/// True for a request that asks to upgrade to WebSocket.
pub fn is_upgrade_request(h: &hudsucker::hyper::HeaderMap) -> bool {
    let has = |name: &str, want: &str| {
        h.get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(want)))
    };
    has("upgrade", "websocket") && has("connection", "upgrade")
}

/// Response headers of the 101 handshake the proxy answers with (plus the connection-id header).
pub fn handshake_response_headers(
    h: &hudsucker::hyper::HeaderMap,
    conn: &str,
) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("upgrade".to_string(), "websocket".to_string());
    m.insert("connection".to_string(), "Upgrade".to_string());
    if let Some(k) = h.get("sec-websocket-key") {
        m.insert(
            "sec-websocket-accept".to_string(),
            hudsucker::tokio_tungstenite::tungstenite::handshake::derive_accept_key(k.as_bytes()),
        );
    }
    m.insert(CONN_HEADER.to_string(), conn.to_string());
    m
}

fn write_if_dirty(frames: &Mutex<Vec<WsFrame>>, file: &std::path::Path, dirty: &std::sync::atomic::AtomicBool) {
    if dirty.swap(false, std::sync::atomic::Ordering::AcqRel) {
        let j = serde_json::to_string(&*frames.lock().unwrap());
        if let Ok(j) = j {
            let _ = std::fs::write(file, j);
        }
    }
}

/// Frames shared by the in-process recorder and the UI (the live feed).
pub fn live_frames() -> Arc<Mutex<Vec<WsFrame>>> {
    static F: OnceLock<Arc<Mutex<Vec<WsFrame>>>> = OnceLock::new();
    F.get_or_init(|| Arc::new(Mutex::new(Vec::new()))).clone()
}

#[derive(Clone)]
pub struct WsRecorder {
    frames: Arc<Mutex<Vec<WsFrame>>>,
    file: PathBuf,
    /// Frames were added since the file was last written.
    dirty: Arc<std::sync::atomic::AtomicBool>,
}

impl WsRecorder {
    /// Recorder that appends to the process-wide live list (shown in the app's Messages window).
    pub fn new(file: PathBuf) -> Self {
        Self::with_frames(file, live_frames())
    }

    /// Recorder with its own frame list (tests, tools). The file is written in the background at
    /// most every 500 ms (rewriting it per message stalled busy sockets); `flush` writes it now.
    pub fn with_frames(file: PathBuf, frames: Arc<Mutex<Vec<WsFrame>>>) -> Self {
        let r = WsRecorder { frames, file, dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        let (weak, file, dirty) = (Arc::downgrade(&r.frames), r.file.clone(), r.dirty.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let Some(frames) = weak.upgrade() else { return };
            write_if_dirty(&frames, &file, &dirty);
        });
        r
    }

    /// Write the frames file now (if anything changed).
    pub fn flush(&self) {
        write_if_dirty(&self.frames, &self.file, &self.dirty);
    }

    fn push(&self, mut f: WsFrame) -> WsFrame {
        let mut l = self.frames.lock().unwrap();
        f.id = l.len() + 1;
        println!(
            "[WS] #{} {} {} {} len={} {}",
            f.id, f.direction, f.url, f.opcode, f.payload_len, f.payload.chars().take(200).collect::<String>()
        );
        l.push(f.clone());
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
        f
    }

    /// Record one message of the connection of `client_addr` to/from `url`; returns the stored frame.
    pub fn record(&self, client_to_server: bool, client_addr: &str, url: &str, message: &Message) -> WsFrame {
        let direction = if client_to_server { "client->server" } else { "server->client" };
        let mut fin = true;
        let (opcode, num, payload, enc, len) = match message {
            Message::Text(t) => {
                let s = t.to_string();
                let n = s.len();
                ("text", 1u8, s, "utf8", n)
            }
            Message::Binary(b) => ("binary", 2, hex(&b[..]), "hex", b.len()),
            Message::Close(c) => {
                let s = match c {
                    Some(f) => format!("{} {}", u16::from(f.code), f.reason),
                    None => String::new(),
                };
                ("close", 8, s, "utf8", 0)
            }
            Message::Ping(b) => ("ping", 9, hex(&b[..]), "hex", b.len()),
            Message::Pong(b) => ("pong", 10, hex(&b[..]), "hex", b.len()),
            Message::Frame(fr) => {
                let h = fr.header();
                fin = h.is_final;
                let n: u8 = h.opcode.into();
                let name = match n {
                    1 => "text",
                    2 => "binary",
                    8 => "close",
                    9 => "ping",
                    10 => "pong",
                    _ => "frame",
                };
                (name, n, hex(fr.payload()), "hex", fr.payload().len())
            }
        };
        self.push(WsFrame {
            id: 0,
            direction: direction.to_string(),
            url: url.to_string(),
            opcode: opcode.to_string(),
            opcode_num: num,
            payload_len: len,
            payload,
            payload_encoding: enc.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            conn: conn_for(client_addr).unwrap_or_default(),
            client: client_addr.to_string(),
            fin,
        })
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Inverse of [`hex`] (whitespace is ignored); None for malformed input.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect()
}

impl WebSocketHandler for WsRecorder {
    async fn handle_message(&mut self, ctx: &WebSocketContext, message: Message) -> Option<Message> {
        let (c2s, client, url) = match ctx {
            WebSocketContext::ClientToServer { src, dst, .. } => (true, src.to_string(), dst.to_string()),
            WebSocketContext::ServerToClient { src, dst, .. } => (false, dst.to_string(), src.to_string()),
        };
        self.record(c2s, &client, &url, &message);
        Some(message)
    }
}

//! G4 streaming protocols: WebSocket / SSE message lists and viewers, Socket.IO and SignalR decoding, gRPC message view.
//! Owned by one builder; the app calls `streams_menu` from the toolbar and `streams_windows` every frame.
//!
//! The "Messages" window follows the grid's selected session:
//! - WebSocket upgrade sessions: message list (direction, type, size, time) + viewer (auto / text / JSON / hex /
//!   MessagePack / Socket.IO / SignalR), handshake headers, metadata, copy message / row, unfragment toggle;
//! - text/event-stream responses: SSE event list (id, event, data, retry) + viewer;
//! - application/grpc bodies: length-prefixed frames with a schema-less protobuf tree.
use egui::{Color32, RichText};

use crate::model::SessionRecord;
use crate::ui::GridApp;
use crate::ws::{WsFrame, CONN_HEADER};

pub use crate::streams_decode::*;

// ================================================================ WebSocket logic

/// True for a WebSocket upgrade session.
pub fn is_websocket_session(s: &SessionRecord) -> bool {
    let up = s.request_headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("upgrade") && v.to_ascii_lowercase().contains("websocket"));
    s.status == 101 || up || s.response_headers.contains_key(CONN_HEADER)
}

/// True for a Server-Sent Events response.
pub fn is_sse_session(s: &SessionRecord) -> bool {
    s.response_headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("text/event-stream"))
}

/// True for a gRPC exchange.
pub fn is_grpc_session(s: &SessionRecord) -> bool {
    let ct = |h: &std::collections::BTreeMap<String, String>| {
        h.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().starts_with("application/grpc"))
    };
    ct(&s.request_headers) || ct(&s.response_headers)
}

/// "host/path?query" of an http(s)/ws(s) URL, lower-case host, default ports dropped (used to link old frames).
pub fn url_key(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(i) => (url[..i].to_ascii_lowercase(), &url[i + 3..]),
        None => (String::new(), url),
    };
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut auth = auth.to_ascii_lowercase();
    let def = if scheme == "https" || scheme == "wss" { ":443" } else { ":80" };
    if auth.ends_with(def) {
        auth.truncate(auth.len() - def.len());
    }
    format!("{auth}{path}")
}

/// The connection id of a WebSocket session (from the proxy's `x-ferrotap-ws-conn` header).
pub fn session_conn(s: &SessionRecord) -> Option<&str> {
    s.response_headers.get(CONN_HEADER).map(|x| x.as_str()).filter(|x| !x.is_empty())
}

/// Frames of a WebSocket session: by connection id when the session has one, otherwise (old captures)
/// the frames without a connection id whose URL matches the session URL.
pub fn frames_for_session<'a>(s: &SessionRecord, frames: &'a [WsFrame]) -> Vec<&'a WsFrame> {
    match session_conn(s) {
        Some(c) => frames.iter().filter(|f| f.conn == c).collect(),
        None => {
            let k = url_key(&s.url);
            frames.iter().filter(|f| f.conn.is_empty() && url_key(&f.url) == k).collect()
        }
    }
}

/// One WebSocket message as shown in the list (a frame, or several frames joined by unfragmenting).
#[derive(Debug, Clone, PartialEq)]
pub struct WsMessage {
    /// 1-based position in the list.
    pub index: usize,
    pub from_client: bool,
    /// text, binary, ping, pong, close, continuation
    pub kind: String,
    pub data: Vec<u8>,
    pub time: String,
    /// Ids of the frames that make this message.
    pub frame_ids: Vec<usize>,
    /// False while a fragmented message has not seen its final frame.
    pub complete: bool,
}

impl WsMessage {
    pub fn is_text(&self) -> bool {
        self.kind == "text" || self.kind == "close"
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    /// Payload as text for text messages, hex for the others.
    pub fn payload_string(&self) -> String {
        if self.is_text() { self.text() } else { crate::ws::hex(&self.data) }
    }

    pub fn direction_label(&self) -> &'static str {
        if self.from_client { "Sent" } else { "Received" }
    }

    /// Tab-separated row: index, direction, type, size, time, payload.
    pub fn row_text(&self) -> String {
        format!("{}\t{}\t{}\t{}\t{}\t{}", self.index, self.direction_label(), self.kind, self.data.len(), self.time, self.payload_string())
    }
}

fn frame_kind(f: &WsFrame) -> &'static str {
    match f.opcode_num {
        0 => "continuation",
        1 => "text",
        2 => "binary",
        8 => "close",
        9 => "ping",
        10 => "pong",
        _ => match f.opcode.as_str() {
            "text" => "text",
            "binary" => "binary",
            "close" => "close",
            "ping" => "ping",
            "pong" => "pong",
            _ => "continuation",
        },
    }
}

/// Turn frames into messages. With `join`, a text/binary frame without FIN and the continuation frames that
/// follow it in the same direction become one message (control frames in between stay separate).
pub fn ws_messages(frames: &[&WsFrame], join: bool) -> Vec<WsMessage> {
    let mut out: Vec<WsMessage> = Vec::new();
    let mut open: [Option<usize>; 2] = [None, None];
    for f in frames {
        let kind = frame_kind(f);
        let d = f.from_client() as usize;
        let data = f.bytes();
        if join {
            if kind == "continuation" {
                if let Some(i) = open[d] {
                    let m = &mut out[i];
                    m.data.extend_from_slice(&data);
                    m.frame_ids.push(f.id);
                    if f.fin {
                        m.complete = true;
                        open[d] = None;
                    }
                    continue;
                }
            } else if (kind == "text" || kind == "binary") && !f.fin {
                open[d] = Some(out.len());
                out.push(WsMessage {
                    index: 0,
                    from_client: f.from_client(),
                    kind: kind.to_string(),
                    data,
                    time: f.timestamp.clone(),
                    frame_ids: vec![f.id],
                    complete: false,
                });
                continue;
            }
        }
        out.push(WsMessage {
            index: 0,
            from_client: f.from_client(),
            kind: kind.to_string(),
            data,
            time: f.timestamp.clone(),
            frame_ids: vec![f.id],
            complete: f.fin || kind == "continuation",
        });
    }
    for (i, m) in out.iter_mut().enumerate() {
        m.index = i + 1;
    }
    out
}

/// Human label for a WebSocket close code (RFC 6455 and the common registered range); "" if unknown.
pub fn close_code_label(code: u16) -> &'static str {
    match code {
        1000 => "Normal Closure",
        1001 => "Going Away",
        1002 => "Protocol Error",
        1003 => "Unsupported Data",
        1004 => "Reserved",
        1005 => "No Status Received",
        1006 => "Abnormal Closure",
        1007 => "Invalid Frame Payload Data",
        1008 => "Policy Violation",
        1009 => "Message Too Big",
        1010 => "Mandatory Extension",
        1011 => "Internal Error",
        1012 => "Service Restart",
        1013 => "Try Again Later",
        1014 => "Bad Gateway",
        1015 => "TLS Handshake",
        3000..=3999 => "Registered (library/framework)",
        4000..=4999 => "Private",
        _ => "",
    }
}

/// Close code and reason of a close frame (recorded text "1000 bye" or raw 2-byte code + reason).
pub fn parse_close(f: &WsFrame) -> (Option<u16>, String) {
    if f.payload_encoding == "hex" {
        let b = f.bytes();
        if b.len() >= 2 {
            return (Some(u16::from_be_bytes([b[0], b[1]])), String::from_utf8_lossy(&b[2..]).into_owned());
        }
        return (None, String::new());
    }
    let p = f.payload.trim();
    let (c, r) = p.split_once(' ').unwrap_or((p, ""));
    match c.parse::<u16>() {
        Ok(code) => (Some(code), r.to_string()),
        Err(_) => (None, p.to_string()),
    }
}

/// Connection metadata shown in the Metadata tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WsStats {
    pub frames: usize,
    pub frames_sent: usize,
    pub frames_received: usize,
    pub bytes_sent: usize,
    pub bytes_received: usize,
    pub first: String,
    pub last: String,
    pub duration_ms: Option<i64>,
    pub close_code: Option<u16>,
    pub close_reason: String,
    /// "client" or "server": who sent the first close frame.
    pub closed_by: String,
}

pub fn ws_stats(frames: &[&WsFrame]) -> WsStats {
    let mut s = WsStats { frames: frames.len(), ..Default::default() };
    for f in frames {
        if f.from_client() {
            s.frames_sent += 1;
            s.bytes_sent += f.payload_len;
        } else {
            s.frames_received += 1;
            s.bytes_received += f.payload_len;
        }
        if frame_kind(f) == "close" && s.closed_by.is_empty() {
            let (c, r) = parse_close(f);
            s.close_code = c;
            s.close_reason = r;
            s.closed_by = if f.from_client() { "client".into() } else { "server".into() };
        }
    }
    if let (Some(a), Some(b)) = (frames.first(), frames.last()) {
        s.first = a.timestamp.clone();
        s.last = b.timestamp.clone();
        let p = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok();
        if let (Some(x), Some(y)) = (p(&a.timestamp), p(&b.timestamp)) {
            s.duration_ms = Some((y - x).num_milliseconds());
        }
    }
    s
}

/// Text the viewer shows for a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Auto,
    Text,
    Json,
    Hex,
    MessagePack,
    SocketIo,
    SignalR,
}

impl ViewMode {
    pub const ALL: [ViewMode; 7] = [
        ViewMode::Auto,
        ViewMode::Text,
        ViewMode::Json,
        ViewMode::Hex,
        ViewMode::MessagePack,
        ViewMode::SocketIo,
        ViewMode::SignalR,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Auto => "Auto",
            ViewMode::Text => "Text",
            ViewMode::Json => "JSON",
            ViewMode::Hex => "Hex",
            ViewMode::MessagePack => "MessagePack",
            ViewMode::SocketIo => "Socket.IO",
            ViewMode::SignalR => "SignalR",
        }
    }
}

/// What the connection speaks, detected from the session URL and its messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConnHints {
    pub socketio: bool,
    pub signalr_json: bool,
    pub signalr_msgpack: bool,
}

pub fn detect_hints(url: &str, msgs: &[WsMessage]) -> ConnHints {
    let mut h = ConnHints { socketio: is_socketio_url(url), ..Default::default() };
    for m in msgs.iter().filter(|m| m.kind == "text").take(4) {
        let t = m.text();
        if let Some(list) = decode_signalr_json(&t) {
            h.signalr_json = true;
            for x in &list {
                if x.kind == "Handshake" && x.detail.get("protocol").and_then(|p| p.as_str()) == Some("messagepack") {
                    h.signalr_msgpack = true;
                    h.signalr_json = false;
                }
            }
        }
    }
    h
}

fn pretty_json(t: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(t.trim()).ok()?;
    serde_json::to_string_pretty(&v).ok()
}

/// Render a message for the viewer; returns (label of the decoder used, text).
pub fn render_message(m: &WsMessage, mode: ViewMode, hints: ConnHints) -> (String, String) {
    let text = m.text();
    match mode {
        ViewMode::Text => ("Text".into(), text),
        ViewMode::Hex => ("Hex".into(), hex_dump(&m.data)),
        ViewMode::Json => match pretty_json(&text) {
            Some(j) => ("JSON".into(), j),
            None => ("JSON (not valid JSON, shown as text)".into(), text),
        },
        ViewMode::MessagePack => match msgpack_to_text(&m.data) {
            Ok(t) => ("MessagePack".into(), t),
            Err(e) => (format!("MessagePack: {e}"), hex_dump(&m.data)),
        },
        ViewMode::SocketIo => match decode_socketio(&text) {
            Some(p) => ("Socket.IO".into(), socketio_to_text(&p)),
            None => ("Socket.IO: not a Socket.IO packet".into(), text),
        },
        ViewMode::SignalR => {
            if m.is_text() {
                match decode_signalr_json(&text) {
                    Some(l) => ("SignalR (JSON hub protocol)".into(), signalr_to_text(&l)),
                    None => ("SignalR: not a JSON hub message".into(), text),
                }
            } else {
                match decode_signalr_msgpack(&m.data) {
                    Some(l) => ("SignalR (MessagePack hub protocol)".into(), signalr_to_text(&l)),
                    None => ("SignalR: not a MessagePack hub message".into(), hex_dump(&m.data)),
                }
            }
        }
        ViewMode::Auto => {
            if m.kind == "close" {
                return ("Close".into(), text);
            }
            if m.is_text() {
                if hints.socketio || looks_like_socketio(&text) {
                    if let Some(p) = decode_socketio(&text) {
                        return ("Socket.IO".into(), socketio_to_text(&p));
                    }
                }
                if let Some(l) = decode_signalr_json(&text) {
                    return ("SignalR (JSON hub protocol)".into(), signalr_to_text(&l));
                }
                if let Some(j) = pretty_json(&text) {
                    return ("JSON".into(), j);
                }
                return ("Text".into(), text);
            }
            if m.kind == "binary" {
                if let Some(l) = decode_signalr_msgpack(&m.data).filter(|_| hints.signalr_msgpack) {
                    return ("SignalR (MessagePack hub protocol)".into(), signalr_to_text(&l));
                }
                if let Ok((v, used)) = mp_decode(&m.data) {
                    if used == m.data.len() && matches!(v, Mp::Map(_) | Mp::Arr(_)) {
                        return ("MessagePack".into(), serde_json::to_string_pretty(&v.to_json()).unwrap_or_default());
                    }
                }
                if let Ok(t) = std::str::from_utf8(&m.data) {
                    if let Some(j) = pretty_json(t) {
                        return ("JSON".into(), j);
                    }
                }
            }
            ("Hex".into(), hex_dump(&m.data))
        }
    }
}

/// gRPC view of a body stored as text: frames + protobuf trees. Err when the body was not kept byte-exact.
pub fn grpc_view(body: &str) -> Result<String, String> {
    if body.contains('\u{fffd}') {
        return Err("This body has bytes that are not UTF-8 and was stored lossily, so its gRPC frames cannot be shown.".into());
    }
    grpc_view_bytes(body.as_bytes())
}

/// gRPC view of the exact captured body bytes.
pub fn grpc_view_bytes(body: &[u8]) -> Result<String, String> {
    let frames = grpc_frames(body)?;
    let mut s = String::new();
    for (i, f) in frames.iter().enumerate() {
        s.push_str(&format!("Message {} - compressed: {}, length: {}\n", i + 1, f.compressed, f.length));
        if f.compressed {
            s.push_str(&hex_dump(&f.data));
        } else {
            match pb_decode(&f.data) {
                Some(t) => s.push_str(&pb_to_text(&t, 1)),
                None => s.push_str(&hex_dump(&f.data)),
            }
        }
    }
    Ok(s)
}

// ================================================================ UI

/// This module's UI state (kept in `GridApp::ext.streams`).
pub struct State {
    /// Messages window shown.
    pub open: bool,
    /// Join continuation frames into whole messages.
    pub unfragment: bool,
    pub mode: ViewMode,
    /// 0 messages, 1 handshake, 2 metadata.
    pub tab: u8,
    /// Selected message index (0-based) in the list of the current session.
    pub selected: Option<usize>,
    /// 0 all, 1 sent only, 2 received only.
    pub dir_filter: u8,
    /// Frames known to the window: loaded from a file and/or copied from the live recorder.
    pub frames: Vec<WsFrame>,
    /// How many live frames were already copied.
    pub live_seen: usize,
    /// Take frames from the in-process recorder.
    pub follow_live: bool,
    pub file_path: String,
    pub status: String,
    last_session: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        State {
            open: false,
            unfragment: true,
            mode: ViewMode::Auto,
            tab: 0,
            selected: None,
            dir_filter: 0,
            frames: Vec::new(),
            live_seen: 0,
            follow_live: true,
            file_path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ws_frames.json").display().to_string(),
            status: String::new(),
            last_session: None,
        }
    }
}

impl State {
    /// Copy new frames from the live recorder.
    pub fn pull_live(&mut self) {
        if !self.follow_live {
            return;
        }
        let live = crate::ws::live_frames();
        let l = live.lock().unwrap();
        if l.len() > self.live_seen {
            self.frames.extend(l[self.live_seen..].iter().cloned());
            self.live_seen = l.len();
        } else if l.len() < self.live_seen {
            self.live_seen = l.len();
        }
    }

    /// Replace the frames with the contents of a ws_frames.json file.
    pub fn load_file(&mut self, path: &std::path::Path) -> Result<usize, String> {
        let f = crate::ws::load_frames(path)?;
        let n = f.len();
        self.frames = f;
        self.live_seen = crate::ws::live_frames().lock().unwrap().len();
        self.selected = None;
        Ok(n)
    }
}

fn short_time(t: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(t) {
        Ok(d) => d.with_timezone(&chrono::Local).format("%H:%M:%S%.3f").to_string(),
        Err(_) => t.to_string(),
    }
}

fn preview(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(80).collect()
}

fn headers_text(h: &std::collections::BTreeMap<String, String>) -> String {
    let mut s = String::new();
    for (k, v) in h {
        s.push_str(&format!("{k}: {v}\n"));
    }
    s
}

/// Read-only multi-line text box (selectable, so parts can be copied).
/// Most text a read-only box shows (laying out megabytes of text takes gigabytes).
const TEXT_BOX_MAX: usize = 200_000;

fn text_box(ui: &mut egui::Ui, id: &str, text: &str, rows: usize) {
    let mut t: &str = text;
    if t.len() > TEXT_BOX_MAX {
        let mut cut = TEXT_BOX_MAX;
        while !t.is_char_boundary(cut) {
            cut -= 1;
        }
        ui.label(RichText::new(format!("Showing the first {cut} of {} bytes (Copy gets all of it).", t.len())).italics().color(Color32::GRAY));
        t = &t[..cut];
    }
    egui::ScrollArea::vertical().id_salt(id).max_height(rows as f32 * 16.0).show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut t)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(rows.min(12)),
        );
    });
}

impl GridApp {
    /// Toolbar entry (a button or a menu) for this module.
    pub fn streams_menu(&mut self, ui: &mut egui::Ui) {
        let st = &mut self.ext.streams;
        if ui
            .selectable_label(st.open, "Messages...")
            .on_hover_text("WebSocket / Server-Sent Events / gRPC messages of the selected session")
            .clicked()
        {
            st.open = !st.open;
        }
    }

    /// Windows this module draws each frame.
    pub fn streams_windows(&mut self, ctx: &egui::Context) {
        if !self.ext.streams.open {
            return;
        }
        self.ext.streams.pull_live();
        let session = self.model.selected_row().cloned();
        let mut open = true;
        egui::Window::new("Messages")
            .open(&mut open)
            .default_size([760.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| self.streams_body(ui, session.as_ref()));
        if !open {
            self.ext.streams.open = false;
        }
    }

    fn streams_body(&mut self, ui: &mut egui::Ui, session: Option<&SessionRecord>) {
        let st = &mut self.ext.streams;
        if st.last_session != session.map(|s| s.id) {
            st.last_session = session.map(|s| s.id);
            st.selected = None;
        }
        ui.horizontal(|ui| {
            ui.label("Frames file:");
            ui.add(egui::TextEdit::singleline(&mut st.file_path).desired_width(300.0));
            if ui.button("Load").on_hover_text("Load a ws_frames.json capture").clicked() {
                let p = std::path::PathBuf::from(st.file_path.trim());
                st.status = match st.load_file(&p) {
                    Ok(n) => format!("Loaded {n} frames"),
                    Err(e) => e,
                };
            }
            ui.checkbox(&mut st.follow_live, "Live").on_hover_text("Add frames captured by this app's proxy");
            if !st.status.is_empty() {
                ui.label(RichText::new(&st.status).color(Color32::GRAY));
            }
        });
        ui.separator();
        let Some(s) = session else {
            ui.label(RichText::new("Select a WebSocket, Server-Sent Events or gRPC session in the grid.").italics());
            return;
        };
        if is_websocket_session(s) {
            self.ws_view(ui, s);
        } else if is_sse_session(s) {
            self.sse_view(ui, s);
        } else if is_grpc_session(s) {
            ui.label(RichText::new(format!("gRPC  #{}  {}", s.id, s.url)).strong());
            self.proto_settings_ui(ui);
            let path = s.path().to_string();
            for (title, body, response) in [("Request messages", s.request_bytes(), false), ("Response messages", s.response_bytes(), true)] {
                ui.label(RichText::new(title).strong());
                let typed = self.ext.proto.schema().map(|sc| crate::protoschema::grpc_view_typed(sc, &path, &body, response));
                match typed {
                    Some(Ok(t)) => text_box(ui, title, &t, 10),
                    other => {
                        if let Some(Err(e)) = other {
                            ui.label(RichText::new(format!("(.proto: {e}; showing field numbers)")).small().color(Color32::GRAY));
                        }
                        match grpc_view_bytes(&body) {
                            Ok(t) => text_box(ui, title, &t, 10),
                            Err(e) => {
                                ui.label(e);
                            }
                        }
                    }
                }
            }
        } else {
            ui.label(format!("Session #{} is not a WebSocket, Server-Sent Events or gRPC session.", s.id));
        }
    }

    fn ws_view(&mut self, ui: &mut egui::Ui, s: &SessionRecord) {
        let st = &mut self.ext.streams;
        let frames = frames_for_session(s, &st.frames);
        let all = ws_messages(&frames, st.unfragment);
        let hints = detect_hints(&s.url, &all);
        ui.label(RichText::new(format!("WebSocket  #{}  {}", s.id, s.url)).strong());
        ui.horizontal(|ui| {
            for (i, t) in ["Messages", "Handshake", "Metadata"].iter().enumerate() {
                if ui.selectable_label(st.tab == i as u8, *t).clicked() {
                    st.tab = i as u8;
                }
            }
            let mut tags = Vec::new();
            if hints.socketio {
                tags.push("Socket.IO");
            }
            if hints.signalr_json {
                tags.push("SignalR JSON");
            }
            if hints.signalr_msgpack {
                tags.push("SignalR MessagePack");
            }
            if !tags.is_empty() {
                ui.label(RichText::new(format!("detected: {}", tags.join(", "))).color(Color32::LIGHT_BLUE));
            }
        });
        ui.separator();
        match st.tab {
            1 => {
                let mut t = format!("{} {} {}\n", s.method, s.url, s.http_version);
                t.push_str(&headers_text(&s.request_headers));
                t.push_str(&format!("\n{} {} Switching Protocols\n", s.response_http_version, s.status));
                t.push_str(&headers_text(&s.response_headers));
                if ui.button("Copy handshake").clicked() {
                    ui.ctx().copy_text(t.clone());
                }
                text_box(ui, "ws_handshake", &t, 24);
            }
            2 => {
                let t = ws_metadata_text(s, &frames);
                if ui.button("Copy metadata").clicked() {
                    ui.ctx().copy_text(t.clone());
                }
                text_box(ui, "ws_meta", &t, 14);
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut st.unfragment, "Unfragment").on_hover_text("Join continuation frames into whole messages");
                    ui.separator();
                    for (i, t) in ["All", "Sent", "Received"].iter().enumerate() {
                        if ui.selectable_label(st.dir_filter == i as u8, *t).clicked() {
                            st.dir_filter = i as u8;
                        }
                    }
                    ui.separator();
                    ui.label(format!("{} messages", all.len()));
                });
                let list: Vec<&WsMessage> = all
                    .iter()
                    .filter(|m| match st.dir_filter {
                        1 => m.from_client,
                        2 => !m.from_client,
                        _ => true,
                    })
                    .collect();
                if list.is_empty() {
                    ui.label(RichText::new("No messages recorded for this connection yet.").italics());
                }
                let mut copy: Option<String> = None;
                let mut clicked: Option<usize> = None;
                let selected = st.selected;
                egui::ScrollArea::vertical().id_salt("ws_list").max_height(220.0).auto_shrink([false, true]).show_rows(
                    ui,
                    18.0,
                    list.len(),
                    |ui, range| {
                        egui::Grid::new("ws_grid").striped(true).num_columns(6).show(ui, |ui| {
                            for h in ["#", "Dir", "Type", "Size", "Time", "Data"] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for i in range {
                                let m = list[i];
                                let sel = selected == Some(m.index - 1);
                                let r = ui.selectable_label(sel, m.index.to_string());
                                let (arrow, col) = if m.from_client {
                                    ("-> sent", Color32::from_rgb(90, 160, 230))
                                } else {
                                    ("<- recv", Color32::from_rgb(90, 190, 120))
                                };
                                ui.label(RichText::new(arrow).color(col));
                                let mut kind = m.kind.clone();
                                if m.frame_ids.len() > 1 {
                                    kind.push_str(&format!(" ({} frames)", m.frame_ids.len()));
                                }
                                if !m.complete {
                                    kind.push_str(" (incomplete)");
                                }
                                ui.label(kind);
                                ui.label(m.data.len().to_string());
                                ui.label(short_time(&m.time));
                                let p = ui.selectable_label(sel, preview(&m.payload_string()));
                                for resp in [r, p] {
                                    if resp.clicked() {
                                        clicked = Some(m.index - 1);
                                    }
                                    resp.context_menu(|ui| {
                                        if ui.button("Copy message").clicked() {
                                            copy = Some(m.payload_string());
                                            ui.close();
                                        }
                                        if ui.button("Copy row").clicked() {
                                            copy = Some(m.row_text());
                                            ui.close();
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                    },
                );
                if clicked.is_some() {
                    st.selected = clicked;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("View as:");
                    for m in ViewMode::ALL {
                        if ui.selectable_label(st.mode == m, m.label()).clicked() {
                            st.mode = m;
                        }
                    }
                });
                match st.selected.and_then(|i| all.get(i)) {
                    None => {
                        ui.label(RichText::new("Select a message to view it.").italics());
                    }
                    Some(m) => {
                        let (label, text) = render_message(m, st.mode, hints);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("#{} {} {} - {} bytes - {}", m.index, m.direction_label(), m.kind, m.data.len(), label)).strong());
                            if ui.button("Copy message").clicked() {
                                copy = Some(m.payload_string());
                            }
                            if ui.button("Copy row").clicked() {
                                copy = Some(m.row_text());
                            }
                            if ui.button("Copy view").clicked() {
                                copy = Some(text.clone());
                            }
                        });
                        text_box(ui, "ws_view", &text, 14);
                    }
                }
                if let Some(c) = copy {
                    ui.ctx().copy_text(c);
                }
            }
        }
    }

    fn sse_view(&mut self, ui: &mut egui::Ui, s: &SessionRecord) {
        let st = &mut self.ext.streams;
        let events = parse_sse(&s.response_body);
        ui.label(RichText::new(format!("Server-Sent Events  #{}  {}  ({} events)", s.id, s.url, events.len())).strong());
        ui.label(
            RichText::new(format!("Events arrived in the response started at {} (no per-event times are recorded).", short_time(&s.start_time)))
                .color(Color32::GRAY),
        );
        let mut copy: Option<String> = None;
        let mut clicked: Option<usize> = None;
        let selected = st.selected;
        egui::ScrollArea::vertical().id_salt("sse_list").max_height(240.0).auto_shrink([false, true]).show_rows(ui, 18.0, events.len(), |ui, range| {
            egui::Grid::new("sse_grid").striped(true).num_columns(6).show(ui, |ui| {
                for h in ["#", "Id", "Event", "Retry", "Time", "Data"] {
                    ui.strong(h);
                }
                ui.end_row();
                for i in range {
                    let e = &events[i];
                    let sel = selected == Some(i);
                    let r = ui.selectable_label(sel, e.index.to_string());
                    ui.label(&e.id);
                    ui.label(&e.event);
                    ui.label(e.retry.map(|r| r.to_string()).unwrap_or_default());
                    ui.label(short_time(&s.start_time));
                    let shown = if e.data.is_empty() && !e.comments.is_empty() { format!(": {}", e.comments.join(" ")) } else { preview(&e.data) };
                    let p = ui.selectable_label(sel, shown);
                    for resp in [r, p] {
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Copy data").clicked() {
                                copy = Some(e.data.clone());
                                ui.close();
                            }
                            if ui.button("Copy row").clicked() {
                                copy = Some(sse_row_text(e));
                                ui.close();
                            }
                        });
                    }
                    ui.end_row();
                }
            });
        });
        if clicked.is_some() {
            st.selected = clicked;
        }
        ui.separator();
        match st.selected.and_then(|i| events.get(i)) {
            None => {
                ui.label(RichText::new("Select an event to view it.").italics());
            }
            Some(e) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Event #{}  {}", e.index, e.event)).strong());
                    if ui.button("Copy data").clicked() {
                        copy = Some(e.data.clone());
                    }
                    if ui.button("Copy row").clicked() {
                        copy = Some(sse_row_text(e));
                    }
                });
                text_box(ui, "sse_view", &sse_event_text(e), 14);
            }
        }
        if let Some(c) = copy {
            ui.ctx().copy_text(c);
        }
    }
}

/// Metadata tab text of a WebSocket session.
pub fn ws_metadata_text(s: &SessionRecord, frames: &[&WsFrame]) -> String {
    let w = ws_stats(frames);
    let msgs = ws_messages(frames, true);
    let mut t = String::new();
    t.push_str(&format!("URL: {}\n", s.url));
    t.push_str(&format!("Connection id: {}\n", session_conn(s).unwrap_or("(none - frames linked by URL)")));
    t.push_str(&format!("Frames: {} ({} sent, {} received)\n", w.frames, w.frames_sent, w.frames_received));
    t.push_str(&format!("Messages (unfragmented): {}\n", msgs.len()));
    t.push_str(&format!("Bytes sent: {}\nBytes received: {}\n", w.bytes_sent, w.bytes_received));
    t.push_str(&format!("Handshake: {}\n", s.start_time));
    t.push_str(&format!("First frame: {}\nLast frame: {}\n", w.first, w.last));
    match w.duration_ms {
        Some(d) => t.push_str(&format!("Duration: {d} ms\n")),
        None => t.push_str("Duration: -\n"),
    }
    match w.close_code {
        Some(c) => {
            let name = close_code_label(c);
            let named = if name.is_empty() { format!("{c}") } else { format!("{c} {name}") };
            t.push_str(&format!("Close: {named} {} (by {})\n", w.close_reason, w.closed_by));
        }
        None if !w.closed_by.is_empty() => t.push_str(&format!("Close: no code (by {})\n", w.closed_by)),
        None => t.push_str("Close: not seen (open, or ended without a close frame)\n"),
    }
    t
}

/// Tab-separated SSE row: index, id, event, retry, data.
pub fn sse_row_text(e: &SseEvent) -> String {
    format!("{}\t{}\t{}\t{}\t{}", e.index, e.id, e.event, e.retry.map(|r| r.to_string()).unwrap_or_default(), e.data)
}

/// Viewer text of an SSE event (data pretty-printed when it is JSON).
pub fn sse_event_text(e: &SseEvent) -> String {
    let mut t = format!("id: {}\nevent: {}\n", e.id, e.event);
    if let Some(r) = e.retry {
        t.push_str(&format!("retry: {r}\n"));
    }
    for c in &e.comments {
        t.push_str(&format!(": {c}\n"));
    }
    t.push_str("\ndata:\n");
    t.push_str(&pretty_json(&e.data).unwrap_or_else(|| e.data.clone()));
    t
}

/// Helpers for tests and automation: show a session without a live proxy.
impl GridApp {
    /// Add a session to the grid model, select it and open the Messages window.
    pub fn streams_show_session(&mut self, s: SessionRecord) {
        let id = s.id;
        self.model.rows.push(s);
        self.model.selected = Some(id);
        self.ext.streams.open = true;
    }

    /// Messages of the selected WebSocket session as the window lists them.
    pub fn streams_selected_messages(&self) -> Vec<WsMessage> {
        let Some(s) = self.model.selected_row() else { return Vec::new() };
        let f = frames_for_session(s, &self.ext.streams.frames);
        ws_messages(&f, self.ext.streams.unfragment)
    }
}

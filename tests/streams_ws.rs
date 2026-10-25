//! G4 WebSocket: frame-to-session linking, old capture files, unfragmenting, metadata, and the Messages window.
use std::sync::{Arc, Mutex};

use ferrotap::model::SessionRecord;
use ferrotap::streams::*;
use ferrotap::ws::{self, WsFrame, WsRecorder};
use hudsucker::hyper::HeaderMap;
use hudsucker::tokio_tungstenite::tungstenite::protocol::frame::coding::{CloseCode, Data, OpCode};
use hudsucker::tokio_tungstenite::tungstenite::protocol::frame::{CloseFrame, Frame};
use hudsucker::tokio_tungstenite::tungstenite::Message;

fn upgrade_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("upgrade", "websocket".parse().unwrap());
    h.insert("connection", "keep-alive, Upgrade".parse().unwrap());
    h.insert("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==".parse().unwrap());
    h.insert("sec-websocket-version", "13".parse().unwrap());
    h
}

/// The session the proxy records for an upgrade request (same calls as proxy.rs).
fn ws_session(id: usize, url: &str, client: &str) -> SessionRecord {
    let h = upgrade_headers();
    assert!(ws::is_upgrade_request(&h));
    let conn = ws::register_upgrade(client);
    SessionRecord {
        id,
        method: "GET".into(),
        url: url.into(),
        status: 101,
        http_version: "HTTP/1.1".into(),
        response_http_version: "HTTP/1.1".into(),
        request_headers: h.iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string())).collect(),
        response_headers: ws::handshake_response_headers(&h, &conn),
        start_time: "2026-01-01T00:00:00+00:00".into(),
        ..Default::default()
    }
}

fn recorder() -> (WsRecorder, Arc<Mutex<Vec<WsFrame>>>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let frames = Arc::new(Mutex::new(Vec::new()));
    (WsRecorder::with_frames(dir.path().join("ws_frames.json"), frames.clone()), frames, dir)
}

#[test]
fn upgrade_detection_and_handshake_headers() {
    let h = upgrade_headers();
    let r = ws::handshake_response_headers(&h, "c1");
    assert_eq!(r["sec-websocket-accept"], "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    assert_eq!(r[ws::CONN_HEADER], "c1");
    let mut plain = HeaderMap::new();
    plain.insert("connection", "keep-alive".parse().unwrap());
    assert!(!ws::is_upgrade_request(&plain));
}

#[test]
fn frames_link_to_their_upgrade_session_by_connection() {
    let (rec, frames, dir) = recorder();
    // two connections to the same URL from different client ports
    let a = ws_session(10, "https://chat.test/ws", "127.0.0.1:50001");
    let b = ws_session(11, "https://chat.test/ws", "127.0.0.1:50002");
    rec.record(true, "127.0.0.1:50001", "wss://chat.test/ws", &Message::Text("hello A".into()));
    rec.record(false, "127.0.0.1:50002", "wss://chat.test/ws", &Message::Text("hello B".into()));
    rec.record(false, "127.0.0.1:50001", "wss://chat.test/ws", &Message::Binary(vec![1u8, 2, 3].into()));
    let all = frames.lock().unwrap().clone();
    assert_eq!(all.len(), 3);
    let fa = frames_for_session(&a, &all);
    let fb = frames_for_session(&b, &all);
    assert_eq!(fa.iter().map(|f| f.id).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(fb.iter().map(|f| f.id).collect::<Vec<_>>(), vec![2]);
    assert!(is_websocket_session(&a));
    // the file the recorder writes loads back with the link
    rec.flush();
    let loaded = ws::load_frames(&dir.path().join("ws_frames.json")).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(frames_for_session(&a, &loaded).len(), 2);
    assert_eq!(loaded[2].bytes(), vec![1, 2, 3]);
}

#[test]
fn old_frames_file_still_loads_and_links_by_url() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ws_frames.json");
    std::fs::write(
        &p,
        r#"[{"id":1,"direction":"client->server","url":"ws://echo.test:80/sock","opcode":"text","opcode_num":1,
             "payload_len":2,"payload":"hi","payload_encoding":"utf8","timestamp":"2026-01-01T00:00:00Z"},
            {"id":2,"direction":"server->client","url":"ws://other.test/x","opcode":"binary","opcode_num":2,
             "payload_len":1,"payload":"ff","payload_encoding":"hex","timestamp":"2026-01-01T00:00:01Z"}]"#,
    )
    .unwrap();
    let f = ws::load_frames(&p).unwrap();
    assert_eq!(f.len(), 2);
    assert!(f[0].fin && f[0].conn.is_empty());
    let s = SessionRecord { id: 1, url: "http://echo.test/sock".into(), status: 101, ..Default::default() };
    let linked = frames_for_session(&s, &f);
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].payload, "hi");
    assert_eq!(url_key("WSS://Host.Test:443/a?b=1"), "host.test/a?b=1");
}

#[test]
fn unfragment_joins_continuation_frames() {
    let (rec, frames, _dir) = recorder();
    let c = "127.0.0.1:50101";
    let s = ws_session(1, "http://frag.test/", c);
    let u = "ws://frag.test/";
    rec.record(true, c, u, &Message::Frame(Frame::message(b"Hel".to_vec(), OpCode::Data(Data::Text), false)));
    rec.record(false, c, u, &Message::Text("interleaved from server".into()));
    rec.record(true, c, u, &Message::Frame(Frame::ping(b"p".to_vec())));
    rec.record(true, c, u, &Message::Frame(Frame::message(b"lo ".to_vec(), OpCode::Data(Data::Continue), false)));
    rec.record(true, c, u, &Message::Frame(Frame::message(b"world".to_vec(), OpCode::Data(Data::Continue), true)));
    rec.record(true, c, u, &Message::Frame(Frame::message(vec![1u8], OpCode::Data(Data::Binary), false)));
    let all = frames.lock().unwrap().clone();
    assert!(!all[0].fin && all[4].fin);
    let f = frames_for_session(&s, &all);
    let raw = ws_messages(&f, false);
    assert_eq!(raw.len(), 6);
    assert_eq!(raw[3].kind, "continuation");
    let m = ws_messages(&f, true);
    assert_eq!(m.len(), 4);
    assert_eq!(m[0].kind, "text");
    assert_eq!(m[0].text(), "Hello world");
    assert_eq!(m[0].frame_ids, vec![1, 4, 5]);
    assert!(m[0].complete);
    assert_eq!(m[1].text(), "interleaved from server");
    assert_eq!(m[2].kind, "ping");
    assert_eq!(m[3].kind, "binary");
    assert!(!m[3].complete, "no final frame yet");
    assert_eq!(m.iter().map(|x| x.index).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert_eq!(m[0].row_text().split('\t').take(4).collect::<Vec<_>>(), vec!["1", "Sent", "text", "11"]);
}

#[test]
fn metadata_counts_bytes_duration_and_close() {
    let mk = |id: usize, dir: &str, op: &str, num: u8, payload: &str, len: usize, ts: &str| WsFrame {
        id,
        direction: dir.into(),
        url: "ws://m.test/".into(),
        opcode: op.into(),
        opcode_num: num,
        payload_len: len,
        payload: payload.into(),
        payload_encoding: "utf8".into(),
        timestamp: ts.into(),
        fin: true,
        ..Default::default()
    };
    let f = [
        mk(1, "client->server", "text", 1, "abcd", 4, "2026-01-01T00:00:00Z"),
        mk(2, "server->client", "text", 1, "xy", 2, "2026-01-01T00:00:01.500Z"),
        mk(3, "server->client", "close", 8, "1001 going away", 0, "2026-01-01T00:00:02Z"),
    ];
    let refs: Vec<&WsFrame> = f.iter().collect();
    let st = ws_stats(&refs);
    assert_eq!((st.frames, st.frames_sent, st.frames_received), (3, 1, 2));
    assert_eq!((st.bytes_sent, st.bytes_received), (4, 2));
    assert_eq!(st.duration_ms, Some(2000));
    assert_eq!(st.close_code, Some(1001));
    assert_eq!(st.close_reason, "going away");
    assert_eq!(st.closed_by, "server");
    let s = SessionRecord { id: 1, url: "http://m.test/".into(), status: 101, ..Default::default() };
    let t = ws_metadata_text(&s, &refs);
    assert!(t.contains("Close: 1001 Going Away going away (by server)") && t.contains("Duration: 2000 ms"), "{t}");
    assert_eq!(close_code_label(1000), "Normal Closure");
    assert_eq!(close_code_label(1011), "Internal Error");
    assert_eq!(close_code_label(4001), "Private");
    assert_eq!(close_code_label(2222), "");
    // a raw close frame recorded via Message::Frame (hex: code + reason)
    let (rec, frames, _d) = recorder();
    rec.record(true, "127.0.0.1:1", "ws://m.test/", &Message::Frame(Frame::close(Some(CloseFrame { code: CloseCode::Normal, reason: "bye".into() }))));
    let fr = frames.lock().unwrap()[0].clone();
    assert_eq!(parse_close(&fr), (Some(1000), "bye".to_string()));
    rec.record(false, "127.0.0.1:1", "ws://m.test/", &Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "later".into() })));
    let fr = frames.lock().unwrap()[1].clone();
    assert_eq!(parse_close(&fr), (Some(1001), "later".to_string()));
}

// ---------------------------------------------------------------- the Messages window

fn collect_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

fn render(app: &mut ferrotap::ui::GridApp, ctx: &egui::Context) -> String {
    let mut text = String::new();
    for _ in 0..3 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.streams_windows(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            collect_text(&s.shape, &mut text);
        }
    }
    text
}

#[test]
fn messages_window_shows_list_viewer_handshake_metadata_and_sse() {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.ext.streams.follow_live = false;
    let c = "127.0.0.1:50201";
    let s = ws_session(77, "https://sio.test/socket.io/?EIO=4&transport=websocket", c);
    let (rec, frames, _d) = recorder();
    let u = "wss://sio.test/socket.io/?EIO=4&transport=websocket";
    rec.record(false, c, u, &Message::Text("0{\"sid\":\"s1\"}".into()));
    rec.record(true, c, u, &Message::Text("42[\"chat\",{\"msg\":\"hey\"}]".into()));
    rec.record(false, c, u, &Message::Binary(vec![0x81u8, 0xa1, 0x61, 0x01].into()));
    app.ext.streams.frames = frames.lock().unwrap().clone();
    app.streams_show_session(s);
    assert_eq!(app.streams_selected_messages().len(), 3);

    let ctx = egui::Context::default();
    let t = render(&mut app, &ctx);
    assert!(t.contains("Messages"), "window title: {t}");
    assert!(t.contains("detected: Socket.IO"), "{t}");
    assert!(t.contains("-> sent") && t.contains("<- recv"), "{t}");
    assert!(t.contains("42[\"chat\""), "row preview: {t}");

    app.ext.streams.selected = Some(1);
    let t = render(&mut app, &ctx);
    assert!(t.contains("Event: chat"), "Socket.IO decode in viewer: {t}");
    app.ext.streams.selected = Some(2);
    let t = render(&mut app, &ctx);
    assert!(t.contains("MessagePack") && t.contains("\"a\": 1"), "MessagePack decode in viewer: {t}");
    app.ext.streams.mode = ViewMode::Hex;
    let t = render(&mut app, &ctx);
    assert!(t.contains("81 a1 61 01"), "{t}");

    app.ext.streams.tab = 1;
    let t = render(&mut app, &ctx);
    assert!(t.contains("sec-websocket-accept") && t.contains("101 Switching Protocols"), "handshake: {t}");
    app.ext.streams.tab = 2;
    let t = render(&mut app, &ctx);
    assert!(t.contains("Frames: 3 (1 sent, 2 received)"), "metadata: {t}");

    // SSE session
    let mut sse = SessionRecord { id: 78, method: "GET".into(), url: "https://sse.test/feed".into(), status: 200, ..Default::default() };
    sse.response_headers.insert("content-type".into(), "text/event-stream".into());
    sse.response_body = "id: 1\nevent: tick\ndata: {\"n\":5}\n\ndata: second\n\n".into();
    app.streams_show_session(sse);
    let t = render(&mut app, &ctx);
    assert!(t.contains("Server-Sent Events") && t.contains("(2 events)") && t.contains("tick"), "{t}");
    app.ext.streams.selected = Some(0);
    let t = render(&mut app, &ctx);
    assert!(t.contains("\"n\": 5"), "SSE viewer: {t}");
}

#[test]
fn busy_socket_does_not_rewrite_the_file_per_message() {
    let (rec, frames, dir) = recorder();
    let t = std::time::Instant::now();
    for i in 0..5000 {
        rec.record(i % 2 == 0, "127.0.0.1:50001", "wss://feed.test/ws", &Message::Text(format!("tick {i} {}", "x".repeat(200)).into()));
    }
    let took = t.elapsed();
    assert!(took < std::time::Duration::from_secs(3), "5000 messages took {took:?}");
    assert_eq!(frames.lock().unwrap().len(), 5000);
    rec.flush();
    assert_eq!(ws::load_frames(&dir.path().join("ws_frames.json")).unwrap().len(), 5000);
}

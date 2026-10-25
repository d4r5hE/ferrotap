//! G4 decoders: SSE, MessagePack, Socket.IO, SignalR (JSON + MessagePack hub protocols), gRPC / protobuf.
use ferrotap::streams::*;

fn unhex(s: &str) -> Vec<u8> {
    ferrotap::ws::unhex(s).expect("hex")
}

// ---------------------------------------------------------------- SSE

#[test]
fn sse_fields_multiline_data_and_defaults() {
    let body = "id: 1\nevent: update\ndata: {\"a\":1}\n\ndata: line1\ndata: line2\n\n: keep-alive\n\nretry: 3000\ndata:x\n\n";
    let ev = parse_sse(body);
    assert_eq!(ev.len(), 4);
    assert_eq!(ev[0].id, "1");
    assert_eq!(ev[0].event, "update");
    assert_eq!(ev[0].data, "{\"a\":1}");
    assert_eq!(ev[1].event, "message", "default event name");
    assert_eq!(ev[1].data, "line1\nline2");
    assert_eq!(ev[1].id, "1", "last event id persists");
    assert_eq!(ev[2].comments, vec!["keep-alive".to_string()]);
    assert_eq!(ev[2].data, "");
    assert_eq!(ev[3].retry, Some(3000));
    assert_eq!(ev[3].data, "x", "no space after the colon");
    assert_eq!(ev.iter().map(|e| e.index).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
}

#[test]
fn sse_crlf_cr_bom_and_unterminated_last_event() {
    let body = "\u{feff}id: 7\r\ndata: a\r\n\r\nevent: e2\rdata: b\r\rdata: tail";
    let ev = parse_sse(body);
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[0].id, "7");
    assert_eq!(ev[0].data, "a");
    assert_eq!(ev[1].event, "e2");
    assert_eq!(ev[1].data, "b");
    assert_eq!(ev[2].data, "tail");
    // invalid retry is ignored, unknown fields too
    let ev = parse_sse("retry: 12a\nfoo: bar\ndata: z\n\n");
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].retry, None);
    assert!(parse_sse("").is_empty());
}

#[test]
fn sse_viewer_text_pretty_prints_json() {
    let ev = parse_sse("id: 9\nevent: tick\nretry: 10\ndata: {\"n\":1}\n\n");
    let t = sse_event_text(&ev[0]);
    assert!(t.contains("id: 9") && t.contains("event: tick") && t.contains("retry: 10"));
    assert!(t.contains("\"n\": 1"), "{t}");
    assert_eq!(sse_row_text(&ev[0]), "1\t9\ttick\t10\t{\"n\":1}");
}

// ---------------------------------------------------------------- MessagePack

#[test]
fn msgpack_map_array_scalars() {
    // {"a":1,"b":[true,nil,-1,"x"],"c":1.5}
    let b = unhex("83a16101a16294c3c0ffa178a163cb3ff8000000000000");
    let (v, used) = mp_decode(&b).unwrap();
    assert_eq!(used, b.len());
    assert_eq!(v.to_json(), serde_json::json!({"a":1,"b":[true,null,-1,"x"],"c":1.5}));
}

#[test]
fn msgpack_wide_types_bin_ext_and_timestamp() {
    // uint16 256, int8 -128, str8 "abc", bin8 dead, int32 -70000, uint64 2^40, float32 0.5
    let b = unhex("97cd0100d080d903616263c402deadd2fffeee90cf0000010000000000ca3f000000");
    let vals = mp_decode_all(&b).unwrap();
    assert_eq!(vals.len(), 1);
    assert_eq!(
        vals[0].to_json(),
        serde_json::json!([256, -128, "abc", {"$bin":"dead"}, -70000, 1099511627776u64, 0.5])
    );
    // fixext4 with type -1 = timestamp 32 (seconds 0)
    let (t, _) = mp_decode(&unhex("d6ff00000000")).unwrap();
    assert_eq!(t.to_json()["$timestamp"].as_str().unwrap(), "1970-01-01T00:00:00+00:00");
    // other ext type
    let (e, _) = mp_decode(&unhex("d40501")).unwrap();
    assert_eq!(e, Mp::Ext(5, vec![1]));
}

#[test]
fn msgpack_16bit_containers_and_errors() {
    // map16 {"k": array16 [1, str16 "hi"]}
    let b = unhex("de0001a16bdc000201da00026869");
    let (v, used) = mp_decode(&b).unwrap();
    assert_eq!(used, b.len());
    assert_eq!(v.to_json(), serde_json::json!({"k":[1,"hi"]}));
    assert!(mp_decode(&unhex("9201")).is_err(), "truncated array");
    assert!(mp_decode(&unhex("c1")).is_err(), "never-used byte");
    assert!(mp_decode_all(&[]).is_err());
    // two values back to back
    assert_eq!(mp_decode_all(&unhex("01a178")).unwrap().len(), 2);
    let txt = msgpack_to_text(&unhex("81a16101")).unwrap();
    assert!(txt.contains("\"a\": 1"));
    // deep nesting is refused, not a stack overflow
    let deep = vec![0x91u8; 10_000];
    assert!(mp_decode(&deep).is_err());
}

// ---------------------------------------------------------------- Socket.IO

#[test]
fn socketio_event_ack_namespace_binary() {
    let p = decode_socketio("42[\"chat\",{\"m\":1},2]").unwrap();
    assert_eq!(p.engine_type, "message");
    assert_eq!(p.socket_type.as_deref(), Some("EVENT"));
    assert_eq!(p.namespace, "/");
    assert_eq!(p.event.as_deref(), Some("chat"));
    assert_eq!(p.args, vec![serde_json::json!({"m":1}), serde_json::json!(2)]);

    let p = decode_socketio("42/admin,7[\"x\",1]").unwrap();
    assert_eq!(p.namespace, "/admin");
    assert_eq!(p.ack_id, Some(7));
    assert_eq!(p.event.as_deref(), Some("x"));

    let p = decode_socketio("431[\"ok\"]").unwrap();
    assert_eq!(p.socket_type.as_deref(), Some("ACK"));
    assert_eq!(p.ack_id, Some(1));
    assert_eq!(p.args, vec![serde_json::json!("ok")]);

    let p = decode_socketio("451-[\"up\",{\"_placeholder\":true,\"num\":0}]").unwrap();
    assert_eq!(p.socket_type.as_deref(), Some("BINARY_EVENT"));
    assert_eq!(p.attachments, 1);
    assert_eq!(p.event.as_deref(), Some("up"));
    let t = socketio_to_text(&p);
    assert!(t.contains("Event: up") && t.contains("Binary attachments: 1"), "{t}");
}

#[test]
fn socketio_engine_packets() {
    let p = decode_socketio("0{\"sid\":\"abc\",\"pingInterval\":25000}").unwrap();
    assert_eq!(p.engine_type, "open");
    assert_eq!(p.data.unwrap()["sid"], "abc");
    let p = decode_socketio("2probe").unwrap();
    assert_eq!(p.engine_type, "ping");
    assert_eq!(p.text, "probe");
    assert_eq!(decode_socketio("3").unwrap().engine_type, "pong");
    let p = decode_socketio("40").unwrap();
    assert_eq!(p.socket_type.as_deref(), Some("CONNECT"));
    let p = decode_socketio("40/admin,{\"sid\":\"x\"}").unwrap();
    assert_eq!(p.namespace, "/admin");
    assert_eq!(p.data.unwrap()["sid"], "x");
    assert!(decode_socketio("hello").is_none());
    assert!(decode_socketio("9x").is_none());
    assert!(decode_socketio("42not json").is_none());
    assert!(is_socketio_url("wss://h/socket.io/?EIO=4&transport=websocket"));
    assert!(!is_socketio_url("wss://h/chat"));
    assert!(looks_like_socketio("42[\"a\"]"));
    assert!(!looks_like_socketio("42"));
}

// ---------------------------------------------------------------- SignalR

#[test]
fn signalr_json_hub_protocol() {
    let t = "{\"type\":1,\"target\":\"Send\",\"invocationId\":\"3\",\"arguments\":[\"hi\",2]}\u{1e}{\"type\":6}\u{1e}";
    let l = decode_signalr_json(t).unwrap();
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].kind, "Invocation");
    assert_eq!(l[0].target.as_deref(), Some("Send"));
    assert_eq!(l[0].invocation_id.as_deref(), Some("3"));
    assert_eq!(l[0].args, vec![serde_json::json!("hi"), serde_json::json!(2)]);
    assert_eq!(l[1].kind, "Ping");
    let c = decode_signalr_json("{\"type\":3,\"invocationId\":\"3\",\"result\":42}\u{1e}").unwrap();
    assert_eq!(c[0].kind, "Completion");
    assert_eq!(c[0].detail["result"], 42);
    let h = decode_signalr_json("{\"protocol\":\"json\",\"version\":1}\u{1e}").unwrap();
    assert_eq!(h[0].kind, "Handshake");
    assert!(decode_signalr_json("{\"type\":1}").is_none(), "needs the 0x1E terminator");
    assert!(decode_signalr_json("not json\u{1e}").is_none());
    let txt = signalr_to_text(&l);
    assert!(txt.contains("Target: Send") && txt.contains("SignalR Ping"), "{txt}");
}

#[test]
fn signalr_messagepack_hub_protocol() {
    // [1, {}, "inv1", "Send", ["hi", 2]]  (18 bytes) + [3, {}, "inv1", 3, 42] (10 bytes) + [6] (2 bytes)
    let b = unhex("129501 80 a4696e7631 a453656e64 92a2686902 0a9503 80 a4696e7631 032a 029106".replace(' ', "").as_str());
    let l = decode_signalr_msgpack(&b).unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!(l[0].kind, "Invocation");
    assert_eq!(l[0].target.as_deref(), Some("Send"));
    assert_eq!(l[0].invocation_id.as_deref(), Some("inv1"));
    assert_eq!(l[0].args, vec![serde_json::json!("hi"), serde_json::json!(2)]);
    assert_eq!(l[1].kind, "Completion");
    assert_eq!(l[1].detail["result"], 42);
    assert_eq!(l[2].kind, "Ping");
    assert!(decode_signalr_msgpack(&unhex("05ff")).is_none(), "length past the end");
    assert!(decode_signalr_msgpack(&unhex("0201ff")).is_none());
    assert!(decode_signalr_msgpack(&unhex("029163")).is_none(), "type 99 is not a hub message");
}

#[test]
fn viewer_auto_mode_picks_the_decoder() {
    let msg = |kind: &str, data: Vec<u8>| WsMessage {
        index: 1,
        from_client: true,
        kind: kind.into(),
        data,
        time: String::new(),
        frame_ids: vec![1],
        complete: true,
    };
    let h = ConnHints { socketio: true, ..Default::default() };
    let (l, t) = render_message(&msg("text", b"42[\"chat\",1]".to_vec()), ViewMode::Auto, h);
    assert_eq!(l, "Socket.IO");
    assert!(t.contains("Event: chat"));
    let (l, _) = render_message(&msg("text", b"{\"type\":6}\x1e".to_vec()), ViewMode::Auto, ConnHints::default());
    assert!(l.starts_with("SignalR"));
    let (l, t) = render_message(&msg("text", b"{\"a\":[1]}".to_vec()), ViewMode::Auto, ConnHints::default());
    assert_eq!(l, "JSON");
    assert!(t.contains("\"a\": ["));
    let (l, _) = render_message(&msg("binary", unhex("81a16101")), ViewMode::Auto, ConnHints::default());
    assert_eq!(l, "MessagePack");
    let sr = ConnHints { signalr_msgpack: true, ..Default::default() };
    let (l, t) = render_message(&msg("binary", unhex("029106")), ViewMode::Auto, sr);
    assert!(l.contains("SignalR"), "{l}");
    assert!(t.contains("Ping"));
    let (l, t) = render_message(&msg("binary", vec![1, 2, 255]), ViewMode::Auto, ConnHints::default());
    assert_eq!(l, "Hex");
    assert!(t.starts_with("00000000  01 02 ff"), "{t}");
    let (_, t) = render_message(&msg("text", b"abc".to_vec()), ViewMode::Hex, ConnHints::default());
    assert!(t.contains("61 62 63") && t.contains("abc"));
}

// ---------------------------------------------------------------- gRPC

#[test]
fn grpc_frames_and_schemaless_protobuf() {
    // 1: 150 (varint), 2: "hi", 3: {1: 1}
    let pb = unhex("089601120268691a020801");
    let mut body = vec![0u8, 0, 0, 0, pb.len() as u8];
    body.extend_from_slice(&pb);
    body.extend_from_slice(&[1, 0, 0, 0, 2, 0xaa, 0xbb]);
    let f = grpc_frames(&body).unwrap();
    assert_eq!(f.len(), 2);
    assert!(!f[0].compressed);
    assert_eq!(f[0].length, pb.len());
    assert!(f[1].compressed);
    let fields = pb_decode(&f[0].data).unwrap();
    assert_eq!(fields[0], PbField { number: 1, wire_type: 0, value: PbValue::Varint(150) });
    assert_eq!(fields[1].value, PbValue::Str("hi".into()));
    assert_eq!(fields[2].value, PbValue::Message(vec![PbField { number: 1, wire_type: 0, value: PbValue::Varint(1) }]));
    let t = pb_to_text(&fields, 0);
    assert!(t.contains("1 (varint): 150") && t.contains("2 (len): \"hi\"") && t.contains("3 (len): {"), "{t}");
    assert!(grpc_frames(&[0, 0]).is_err());
    assert!(grpc_frames(&[7, 0, 0, 0, 0]).is_err());
    // body kept as text in a session: works when the bytes are UTF-8, refused otherwise
    let ok = String::from_utf8(vec![0, 0, 0, 0, 2, 0x08, 0x05]).unwrap();
    assert!(grpc_view(&ok).unwrap().contains("1 (varint): 5"));
    assert!(grpc_view("\u{fffd}").is_err());
}

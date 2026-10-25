//! G4 decoders used by the Messages window: MessagePack, Server-Sent Events, Socket.IO (engine.io +
//! socket.io packets), SignalR (JSON and MessagePack hub protocols), gRPC length-prefixed frames with a
//! schema-less protobuf view, and a hex dump. Pure functions, no UI.
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------- hex dump

/// Classic hex dump: offset, 16 bytes in hex, printable ASCII.
pub fn hex_dump(b: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in b.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(x) => out.push_str(&format!("{x:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for &x in chunk {
            out.push(if (0x20..0x7f).contains(&x) { x as char } else { '.' });
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- MessagePack

/// A decoded MessagePack value.
#[derive(Debug, Clone, PartialEq)]
pub enum Mp {
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
    Str(String),
    Bin(Vec<u8>),
    Arr(Vec<Mp>),
    Map(Vec<(Mp, Mp)>),
    Ext(i8, Vec<u8>),
}

const MP_MAX_DEPTH: usize = 100;

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.b.len() - self.p < n {
            return Err(format!("truncated at offset {} (need {n} bytes)", self.p));
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn be(&mut self, n: usize) -> Result<u64, String> {
        Ok(self.take(n)?.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
    }
}

fn mp_value(r: &mut Rd, depth: usize) -> Result<Mp, String> {
    if depth > MP_MAX_DEPTH {
        return Err("nesting too deep".into());
    }
    let t = r.u8()?;
    let str_of = |r: &mut Rd, n: usize| -> Result<Mp, String> {
        let s = r.take(n)?;
        Ok(Mp::Str(String::from_utf8_lossy(s).into_owned()))
    };
    let arr = |r: &mut Rd, n: usize| -> Result<Mp, String> {
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push(mp_value(r, depth + 1)?);
        }
        Ok(Mp::Arr(v))
    };
    let map = |r: &mut Rd, n: usize| -> Result<Mp, String> {
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let k = mp_value(r, depth + 1)?;
            let x = mp_value(r, depth + 1)?;
            v.push((k, x));
        }
        Ok(Mp::Map(v))
    };
    let ext = |r: &mut Rd, n: usize| -> Result<Mp, String> {
        let ty = r.u8()? as i8;
        Ok(Mp::Ext(ty, r.take(n)?.to_vec()))
    };
    Ok(match t {
        0x00..=0x7f => Mp::UInt(t as u64),
        0x80..=0x8f => return map(r, (t & 0x0f) as usize),
        0x90..=0x9f => return arr(r, (t & 0x0f) as usize),
        0xa0..=0xbf => return str_of(r, (t & 0x1f) as usize),
        0xc0 => Mp::Nil,
        0xc2 => Mp::Bool(false),
        0xc3 => Mp::Bool(true),
        0xc4..=0xc6 => {
            let n = r.be(1 << (t - 0xc4))? as usize;
            Mp::Bin(r.take(n)?.to_vec())
        }
        0xc7..=0xc9 => {
            let n = r.be(1 << (t - 0xc7))? as usize;
            return ext(r, n);
        }
        0xca => Mp::F32(f32::from_bits(r.be(4)? as u32)),
        0xcb => Mp::F64(f64::from_bits(r.be(8)?)),
        0xcc..=0xcf => Mp::UInt(r.be(1 << (t - 0xcc))?),
        0xd0 => Mp::Int(r.be(1)? as u8 as i8 as i64),
        0xd1 => Mp::Int(r.be(2)? as u16 as i16 as i64),
        0xd2 => Mp::Int(r.be(4)? as u32 as i32 as i64),
        0xd3 => Mp::Int(r.be(8)? as i64),
        0xd4..=0xd8 => return ext(r, 1 << (t - 0xd4)),
        0xd9..=0xdb => {
            let n = r.be(1 << (t - 0xd9))? as usize;
            return str_of(r, n);
        }
        0xdc | 0xdd => {
            let n = r.be(2 << (t - 0xdc))? as usize;
            return arr(r, n);
        }
        0xde | 0xdf => {
            let n = r.be(2 << (t - 0xde))? as usize;
            return map(r, n);
        }
        0xe0..=0xff => Mp::Int(t as i8 as i64),
        0xc1 => return Err(format!("invalid type byte 0xc1 at offset {}", r.p - 1)),
    })
}

/// Decode one MessagePack value from the start of `b`; returns it and the number of bytes used.
pub fn mp_decode(b: &[u8]) -> Result<(Mp, usize), String> {
    let mut r = Rd { b, p: 0 };
    let v = mp_value(&mut r, 0)?;
    Ok((v, r.p))
}

/// Decode a sequence of MessagePack values that fills `b` exactly.
pub fn mp_decode_all(b: &[u8]) -> Result<Vec<Mp>, String> {
    let mut r = Rd { b, p: 0 };
    let mut out = Vec::new();
    while r.p < b.len() {
        out.push(mp_value(&mut r, 0)?);
    }
    if out.is_empty() {
        return Err("empty".into());
    }
    Ok(out)
}

impl Mp {
    /// JSON view: bin -> {"$bin": hex}, ext -> {"$ext": type, "data": hex} (timestamp ext -1 -> {"$timestamp": rfc3339}),
    /// non-string map keys are written in their JSON text form.
    pub fn to_json(&self) -> Value {
        match self {
            Mp::Nil => Value::Null,
            Mp::Bool(b) => json!(b),
            Mp::Int(i) => json!(i),
            Mp::UInt(u) => json!(u),
            Mp::F32(f) => json!(*f as f64),
            Mp::F64(f) => json!(f),
            Mp::Str(s) => json!(s),
            Mp::Bin(b) => json!({ "$bin": crate::ws::hex(b) }),
            Mp::Arr(a) => Value::Array(a.iter().map(|x| x.to_json()).collect()),
            Mp::Map(m) => {
                let mut o = Map::new();
                for (k, v) in m {
                    let key = match k {
                        Mp::Str(s) => s.clone(),
                        other => other.to_json().to_string(),
                    };
                    o.insert(key, v.to_json());
                }
                Value::Object(o)
            }
            Mp::Ext(-1, d) => match mp_timestamp(d) {
                Some(t) => json!({ "$timestamp": t }),
                None => json!({ "$ext": -1, "data": crate::ws::hex(d) }),
            },
            Mp::Ext(t, d) => json!({ "$ext": t, "data": crate::ws::hex(d) }),
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Mp::Int(i) => Some(*i),
            Mp::UInt(u) => i64::try_from(*u).ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Mp::Str(s) => Some(s),
            _ => None,
        }
    }
}

fn mp_timestamp(d: &[u8]) -> Option<String> {
    let be = |s: &[u8]| s.iter().fold(0u64, |a, &x| (a << 8) | x as u64);
    let (secs, nanos): (i64, u32) = match d.len() {
        4 => (be(d) as i64, 0),
        8 => {
            let v = be(d);
            ((v & 0x3_ffff_ffff) as i64, (v >> 34) as u32)
        }
        12 => (be(&d[4..]) as i64, be(&d[..4]) as u32),
        _ => return None,
    };
    chrono::DateTime::from_timestamp(secs, nanos).map(|t| t.to_rfc3339())
}

/// Pretty JSON text of one or more MessagePack values filling `b`.
pub fn msgpack_to_text(b: &[u8]) -> Result<String, String> {
    let vals = mp_decode_all(b)?;
    let v: Vec<Value> = vals.iter().map(|x| x.to_json()).collect();
    let shown = if v.len() == 1 { v[0].clone() } else { Value::Array(v) };
    Ok(serde_json::to_string_pretty(&shown).unwrap_or_default())
}

// ---------------------------------------------------------------- Server-Sent Events

/// One dispatched Server-Sent Event.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SseEvent {
    /// 1-based position in the stream.
    pub index: usize,
    /// Last event id in effect when the event was dispatched (the `id:` field persists, per the spec).
    pub id: String,
    /// `event:` field, "message" when absent.
    pub event: String,
    /// `data:` lines joined with '\n'.
    pub data: String,
    /// `retry:` field of this block, if any.
    pub retry: Option<u64>,
    /// `:` comment lines of this block.
    pub comments: Vec<String>,
}

/// Parse a text/event-stream body. Blocks are separated by blank lines; a block is listed when it has
/// data, an event name, an id, a retry value or a comment (so keep-alive comments are visible too).
pub fn parse_sse(body: &str) -> Vec<SseEvent> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let mut out = Vec::new();
    let mut last_id = String::new();
    let mut ev = String::new();
    let mut data: Vec<String> = Vec::new();
    let mut has_data = false;
    let mut retry = None;
    let mut comments = Vec::new();
    let mut any = false;
    let norm = body.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = norm.split('\n').collect();
    lines.push(""); // a stream cut mid-block still shows its last event
    for line in lines {
        if line.is_empty() {
            if any {
                out.push(SseEvent {
                    index: out.len() + 1,
                    id: last_id.clone(),
                    event: if ev.is_empty() { "message".into() } else { ev.clone() },
                    data: if has_data { data.join("\n") } else { String::new() },
                    retry,
                    comments: std::mem::take(&mut comments),
                });
            }
            ev.clear();
            data.clear();
            has_data = false;
            retry = None;
            any = false;
            continue;
        }
        if let Some(c) = line.strip_prefix(':') {
            comments.push(c.strip_prefix(' ').unwrap_or(c).to_string());
            any = true;
            continue;
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "event" => {
                ev = value.to_string();
                any = true;
            }
            "data" => {
                data.push(value.to_string());
                has_data = true;
                any = true;
            }
            "id" => {
                if !value.contains('\0') {
                    last_id = value.to_string();
                }
                any = true;
            }
            "retry"
                if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) => {
                    retry = value.parse().ok();
                    any = true;
                }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------- Socket.IO

/// A decoded engine.io / socket.io packet (text form, as sent over WebSocket).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SioPacket {
    /// open, close, ping, pong, message, upgrade, noop
    pub engine_type: String,
    /// CONNECT, DISCONNECT, EVENT, ACK, CONNECT_ERROR, BINARY_EVENT, BINARY_ACK (engine type "message" only)
    pub socket_type: Option<String>,
    pub namespace: String,
    pub ack_id: Option<u64>,
    pub attachments: usize,
    /// EVENT / BINARY_EVENT name (first array element).
    pub event: Option<String>,
    /// Remaining event args, or the ACK args.
    pub args: Vec<Value>,
    /// JSON payload of open / CONNECT / CONNECT_ERROR packets.
    pub data: Option<Value>,
    /// Text after the engine type for packets without JSON (e.g. "probe").
    pub text: String,
}

const EIO_TYPES: [&str; 7] = ["open", "close", "ping", "pong", "message", "upgrade", "noop"];
const SIO_TYPES: [&str; 7] = ["CONNECT", "DISCONNECT", "EVENT", "ACK", "CONNECT_ERROR", "BINARY_EVENT", "BINARY_ACK"];

/// True when a session URL is a Socket.IO endpoint.
pub fn is_socketio_url(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.contains("/socket.io/") || u.contains("eio=")
}

/// Decode one engine.io text packet (and the socket.io packet inside a "message").
pub fn decode_socketio(text: &str) -> Option<SioPacket> {
    let mut chars = text.chars();
    let et = chars.next()?.to_digit(10)? as usize;
    let mut p = SioPacket { engine_type: EIO_TYPES.get(et)?.to_string(), ..Default::default() };
    let rest = chars.as_str();
    if et == 0 {
        p.data = serde_json::from_str(rest).ok();
        p.text = rest.to_string();
        return Some(p);
    }
    if et != 4 {
        p.text = rest.to_string();
        return Some(p);
    }
    let mut c = rest.char_indices().peekable();
    let st = match c.next() {
        Some((_, ch)) => ch.to_digit(10)? as usize,
        None => {
            p.text = String::new();
            return Some(p);
        }
    };
    p.socket_type = Some(SIO_TYPES.get(st)?.to_string());
    let mut i = 1usize;
    let b = rest.as_bytes();
    if st == 5 || st == 6 {
        let s = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'-' {
            return None;
        }
        p.attachments = rest[s..i].parse().ok()?;
        i += 1;
    }
    if i < b.len() && b[i] == b'/' {
        let s = i;
        while i < b.len() && b[i] != b',' {
            i += 1;
        }
        p.namespace = rest[s..i].to_string();
        if i < b.len() {
            i += 1;
        }
    } else {
        p.namespace = "/".into();
    }
    let s = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > s {
        p.ack_id = rest[s..i].parse().ok();
    }
    let payload = &rest[i..];
    if payload.is_empty() {
        return Some(p);
    }
    let v: Value = serde_json::from_str(payload).ok()?;
    match st {
        2 | 5 => {
            let arr = v.as_array()?;
            p.event = arr.first().and_then(|x| x.as_str()).map(|x| x.to_string());
            p.args = arr.iter().skip(1).cloned().collect();
        }
        3 | 6 => p.args = v.as_array().cloned().unwrap_or_else(|| vec![v]),
        _ => p.data = Some(v),
    }
    Some(p)
}

/// Heuristic for text messages of sessions whose URL does not say socket.io: a socket.io EVENT/ACK.
pub fn looks_like_socketio(text: &str) -> bool {
    matches!(decode_socketio(text), Some(SioPacket { socket_type: Some(ref t), .. }) if t.contains("EVENT") || t.contains("ACK"))
        && text.contains('[')
}

/// Readable multi-line description of a Socket.IO packet.
pub fn socketio_to_text(p: &SioPacket) -> String {
    let mut s = format!("Engine.IO packet: {}\n", p.engine_type);
    if let Some(t) = &p.socket_type {
        s.push_str(&format!("Socket.IO packet: {t}\nNamespace: {}\n", p.namespace));
    }
    if let Some(a) = p.ack_id {
        s.push_str(&format!("Ack id: {a}\n"));
    }
    if p.attachments > 0 {
        s.push_str(&format!("Binary attachments: {} (sent as the next binary messages)\n", p.attachments));
    }
    if let Some(e) = &p.event {
        s.push_str(&format!("Event: {e}\n"));
    }
    for (i, a) in p.args.iter().enumerate() {
        s.push_str(&format!("Arg [{i}]: {}\n", serde_json::to_string_pretty(a).unwrap_or_default()));
    }
    if let Some(d) = &p.data {
        s.push_str(&format!("Data: {}\n", serde_json::to_string_pretty(d).unwrap_or_default()));
    } else if !p.text.is_empty() {
        s.push_str(&format!("Data: {}\n", p.text));
    }
    s
}

// ---------------------------------------------------------------- SignalR

pub const SIGNALR_RS: char = '\u{1e}';

/// One SignalR hub-protocol message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SrMessage {
    /// Invocation, StreamItem, Completion, StreamInvocation, CancelInvocation, Ping, Close, Ack, Sequence,
    /// Handshake (the client's {"protocol":..} or the server's {} / {"error":..}).
    pub kind: String,
    pub type_num: Option<i64>,
    pub target: Option<String>,
    pub invocation_id: Option<String>,
    pub args: Vec<Value>,
    /// Item / result / error / handshake body / other fields.
    pub detail: Value,
}

pub fn signalr_type_name(n: i64) -> &'static str {
    match n {
        1 => "Invocation",
        2 => "StreamItem",
        3 => "Completion",
        4 => "StreamInvocation",
        5 => "CancelInvocation",
        6 => "Ping",
        7 => "Close",
        8 => "Ack",
        9 => "Sequence",
        _ => "Unknown",
    }
}

/// Decode a text message of the JSON hub protocol (records terminated by 0x1E). None when the text is
/// not a list of 0x1E-terminated JSON objects.
pub fn decode_signalr_json(text: &str) -> Option<Vec<SrMessage>> {
    if !text.ends_with(SIGNALR_RS) {
        return None;
    }
    let mut out = Vec::new();
    for rec in text.split(SIGNALR_RS).filter(|r| !r.trim().is_empty()) {
        let v: Value = serde_json::from_str(rec).ok()?;
        let o = v.as_object()?;
        let mut m = SrMessage::default();
        match o.get("type").and_then(|t| t.as_i64()) {
            None => {
                m.kind = "Handshake".into();
                m.detail = v.clone();
            }
            Some(t) => {
                m.type_num = Some(t);
                m.kind = signalr_type_name(t).into();
                m.target = o.get("target").and_then(|x| x.as_str()).map(String::from);
                m.invocation_id = o.get("invocationId").and_then(|x| x.as_str()).map(String::from);
                m.args = o.get("arguments").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                let mut rest = o.clone();
                for k in ["type", "target", "invocationId", "arguments"] {
                    rest.remove(k);
                }
                m.detail = Value::Object(rest);
            }
        }
        out.push(m);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Decode a binary message of the MessagePack hub protocol: each message is a VarInt length prefix
/// followed by a MessagePack array [type, headers, ...]. None when the bytes do not fit that shape.
pub fn decode_signalr_msgpack(b: &[u8]) -> Option<Vec<SrMessage>> {
    let mut p = 0usize;
    let mut out = Vec::new();
    while p < b.len() {
        let mut len = 0usize;
        let mut shift = 0;
        loop {
            let x = *b.get(p)?;
            p += 1;
            len |= ((x & 0x7f) as usize) << shift;
            if x & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return None;
            }
        }
        let body = b.get(p..p.checked_add(len)?)?;
        p += len;
        let (v, used) = mp_decode(body).ok()?;
        if used != body.len() {
            return None;
        }
        let Mp::Arr(a) = v else { return None };
        let t = a.first()?.as_i64()?;
        if !(1..=9).contains(&t) {
            return None;
        }
        let mut m = SrMessage { kind: signalr_type_name(t).into(), type_num: Some(t), ..Default::default() };
        let id = |x: Option<&Mp>| x.and_then(|v| v.as_str()).map(String::from);
        let args = |x: Option<&Mp>| match x {
            Some(Mp::Arr(v)) => v.iter().map(|y| y.to_json()).collect(),
            _ => Vec::new(),
        };
        match t {
            1 | 4 => {
                m.invocation_id = id(a.get(2));
                m.target = id(a.get(3));
                m.args = args(a.get(4));
                if let Some(s) = a.get(5) {
                    m.detail = json!({ "streamIds": s.to_json() });
                }
            }
            2 => {
                m.invocation_id = id(a.get(2));
                m.detail = json!({ "item": a.get(3).map(|x| x.to_json()) });
            }
            3 => {
                m.invocation_id = id(a.get(2));
                let kind = a.get(3).and_then(|x| x.as_i64()).unwrap_or(2);
                m.detail = match kind {
                    1 => json!({ "error": a.get(4).map(|x| x.to_json()) }),
                    3 => json!({ "result": a.get(4).map(|x| x.to_json()) }),
                    _ => json!({ "result": null }),
                };
            }
            5 => m.invocation_id = id(a.get(2)),
            7 => {
                m.detail = json!({
                    "error": a.get(1).map(|x| x.to_json()),
                    "allowReconnect": a.get(2).map(|x| x.to_json()),
                })
            }
            8 | 9 => m.detail = json!({ "sequenceId": a.get(1).map(|x| x.to_json()) }),
            _ => {}
        }
        out.push(m);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Readable multi-line description of SignalR messages.
pub fn signalr_to_text(list: &[SrMessage]) -> String {
    let mut s = String::new();
    for (i, m) in list.iter().enumerate() {
        if list.len() > 1 {
            s.push_str(&format!("--- message {} ---\n", i + 1));
        }
        match m.type_num {
            Some(t) => s.push_str(&format!("SignalR {} (type {t})\n", m.kind)),
            None => s.push_str(&format!("SignalR {}\n", m.kind)),
        }
        if let Some(t) = &m.target {
            s.push_str(&format!("Target: {t}\n"));
        }
        if let Some(id) = &m.invocation_id {
            s.push_str(&format!("Invocation id: {id}\n"));
        }
        for (j, a) in m.args.iter().enumerate() {
            s.push_str(&format!("Arg [{j}]: {}\n", serde_json::to_string_pretty(a).unwrap_or_default()));
        }
        let empty = matches!(&m.detail, Value::Null) || matches!(&m.detail, Value::Object(o) if o.is_empty() && m.type_num.is_some());
        if !empty {
            s.push_str(&format!("{}\n", serde_json::to_string_pretty(&m.detail).unwrap_or_default()));
        }
    }
    s
}

// ---------------------------------------------------------------- gRPC + protobuf

/// One gRPC length-prefixed message.
#[derive(Debug, Clone, PartialEq)]
pub struct GrpcFrame {
    pub compressed: bool,
    pub length: usize,
    pub data: Vec<u8>,
}

/// Split a gRPC body into its 5-byte-prefixed messages. A truncated last frame is returned with the bytes
/// present; Err for a body that does not start with a valid prefix.
pub fn grpc_frames(b: &[u8]) -> Result<Vec<GrpcFrame>, String> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        if b.len() - p < 5 {
            return Err(format!("{} trailing bytes are not a gRPC frame prefix", b.len() - p));
        }
        let flag = b[p];
        if flag > 1 {
            return Err(format!("bad compressed flag {flag} at offset {p}"));
        }
        let len = u32::from_be_bytes([b[p + 1], b[p + 2], b[p + 3], b[p + 4]]) as usize;
        let end = (p + 5).saturating_add(len).min(b.len());
        out.push(GrpcFrame { compressed: flag == 1, length: len, data: b[p + 5..end].to_vec() });
        p = end;
    }
    Ok(out)
}

/// A protobuf field decoded without a schema.
#[derive(Debug, Clone, PartialEq)]
pub enum PbValue {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    /// Length-delimited: nested message when it parses as one, else string/bytes.
    Message(Vec<PbField>),
    Str(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PbField {
    pub number: u64,
    pub wire_type: u8,
    pub value: PbValue,
}

fn pb_varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..10 {
        let x = *b.get(*p)?;
        *p += 1;
        v |= ((x & 0x7f) as u64) << (7 * i);
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Decode protobuf bytes into a field-number / wire-type tree; None when they are not valid protobuf.
pub fn pb_decode(b: &[u8]) -> Option<Vec<PbField>> {
    pb_decode_depth(b, 0)
}

fn pb_decode_depth(b: &[u8], depth: usize) -> Option<Vec<PbField>> {
    let mut p = 0;
    let mut out = Vec::new();
    while p < b.len() {
        let key = pb_varint(b, &mut p)?;
        let number = key >> 3;
        let wt = (key & 7) as u8;
        if number == 0 {
            return None;
        }
        let value = match wt {
            0 => PbValue::Varint(pb_varint(b, &mut p)?),
            1 => {
                let s = b.get(p..p + 8)?;
                p += 8;
                PbValue::Fixed64(u64::from_le_bytes(s.try_into().ok()?))
            }
            5 => {
                let s = b.get(p..p + 4)?;
                p += 4;
                PbValue::Fixed32(u32::from_le_bytes(s.try_into().ok()?))
            }
            2 => {
                let n = pb_varint(b, &mut p)? as usize;
                let s = b.get(p..p.checked_add(n)?)?;
                p += n;
                let nested = if depth < 32 && !s.is_empty() { pb_decode_depth(s, depth + 1) } else { None };
                match (nested, std::str::from_utf8(s)) {
                    (_, Ok(t)) if !t.is_empty() && t.chars().all(|c| !c.is_control() || c == '\n' || c == '\t') => {
                        PbValue::Str(t.to_string())
                    }
                    (Some(f), _) => PbValue::Message(f),
                    (None, Ok(t)) => PbValue::Str(t.to_string()),
                    (None, Err(_)) => PbValue::Bytes(s.to_vec()),
                }
            }
            _ => return None,
        };
        out.push(PbField { number, wire_type: wt, value });
    }
    Some(out)
}

/// Indented text tree of decoded protobuf fields.
pub fn pb_to_text(fields: &[PbField], indent: usize) -> String {
    let mut s = String::new();
    let pad = "  ".repeat(indent);
    for f in fields {
        let wt = match f.wire_type {
            0 => "varint",
            1 => "fixed64",
            2 => "len",
            5 => "fixed32",
            _ => "?",
        };
        match &f.value {
            PbValue::Varint(v) => s.push_str(&format!("{pad}{} ({wt}): {v}\n", f.number)),
            PbValue::Fixed64(v) => s.push_str(&format!("{pad}{} ({wt}): {v} / {}\n", f.number, f64::from_bits(*v))),
            PbValue::Fixed32(v) => s.push_str(&format!("{pad}{} ({wt}): {v} / {}\n", f.number, f32::from_bits(*v))),
            PbValue::Str(t) => s.push_str(&format!("{pad}{} ({wt}): {:?}\n", f.number, t)),
            PbValue::Bytes(b) => s.push_str(&format!("{pad}{} ({wt}): bytes {}\n", f.number, crate::ws::hex(b))),
            PbValue::Message(m) => {
                s.push_str(&format!("{pad}{} ({wt}): {{\n", f.number));
                s.push_str(&pb_to_text(m, indent + 1));
                s.push_str(&format!("{pad}}}\n"));
            }
        }
    }
    s
}

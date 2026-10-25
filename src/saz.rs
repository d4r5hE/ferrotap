//! SAZ session archives (the zip layout Fiddler uses): `raw/NN_c.txt` (request as sent),
//! `raw/NN_s.txt` (response as received) and `raw/NN_m.xml` (timers + flags).
//! Our own small zip reader/writer (stored or deflate entries) on top of flate2.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use crate::model::{SessionMark, SessionRecord};

// ------------------------------------------------------------------------------------------ zip

fn u16le(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2).map(|x| u16::from_le_bytes([x[0], x[1]])).ok_or_else(|| "zip: truncated".to_string())
}

fn u32le(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).ok_or_else(|| "zip: truncated".to_string())
}

/// Read every file entry of a zip archive (name -> bytes). Supports stored and deflate entries.
pub fn zip_read(data: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    const EOCD: u32 = 0x0605_4b50;
    let min = data.len().saturating_sub(22 + 65_535);
    let eocd = (min..=data.len().saturating_sub(22))
        .rev()
        .find(|&i| u32le(data, i).ok() == Some(EOCD))
        .ok_or("not a zip file (no end of central directory)")?;
    let count = u16le(data, eocd + 10)? as usize;
    let mut at = u32le(data, eocd + 16)? as usize;
    let mut out = BTreeMap::new();
    for _ in 0..count {
        if u32le(data, at)? != 0x0201_4b50 {
            return Err("zip: bad central directory".into());
        }
        let method = u16le(data, at + 10)?;
        let csize = u32le(data, at + 20)? as usize;
        let name_len = u16le(data, at + 28)? as usize;
        let extra_len = u16le(data, at + 30)? as usize;
        let comment_len = u16le(data, at + 32)? as usize;
        let local = u32le(data, at + 42)? as usize;
        let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + name_len).ok_or("zip: truncated")?).replace('\\', "/");
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        if u32le(data, local)? != 0x0403_4b50 {
            return Err(format!("zip: bad local header for {name}"));
        }
        let start = local + 30 + u16le(data, local + 26)? as usize + u16le(data, local + 28)? as usize;
        let raw = data.get(start..start + csize).ok_or_else(|| format!("zip: {name} is truncated"))?;
        let bytes = match method {
            0 => raw.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(raw).take(512 * 1024 * 1024).read_to_end(&mut v).map_err(|e| format!("zip: inflate {name}: {e}"))?;
                v
            }
            m => return Err(format!("zip: {name} uses unsupported compression method {m}")),
        };
        out.insert(name, bytes);
    }
    Ok(out)
}

/// Write a zip archive (deflate entries) from (name, bytes) pairs.
pub fn zip_write(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).map_err(|e| e.to_string())?;
        let comp = enc.finish().map_err(|e| e.to_string())?;
        let (offset, n) = (out.len() as u32, name.as_bytes());
        let head = |sig: u32, central_entry: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central_entry {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
            h.extend_from_slice(&8u16.to_le_bytes()); // deflate
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
            h.extend_from_slice(&crc.sum().to_le_bytes());
            h.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(n.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central_entry {
                h.extend_from_slice(&[0u8; 6]); // comment len, disk, internal attrs
                h.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(n);
            h
        };
        let local = head(0x0403_4b50, false);
        let cent = head(0x0201_4b50, true);
        out.extend_from_slice(&local);
        out.extend_from_slice(&comp);
        central.extend_from_slice(&cent);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

// ------------------------------------------------------------------------------------------ http text

fn split_head(b: &[u8]) -> (&[u8], &[u8]) {
    match b.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => (&b[..i], &b[i + 4..]),
        None => match b.windows(2).position(|w| w == b"\n\n") {
            Some(i) => (&b[..i], &b[i + 2..]),
            None => (b, &[]),
        },
    }
}

/// Header lines into a map; repeated names joined like the proxy does (Set-Cookie by newline).
fn parse_headers<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<String, String> {
    let mut h: BTreeMap<String, String> = BTreeMap::new();
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
        match h.get_mut(&k) {
            Some(prev) => {
                prev.push_str(if k == "set-cookie" { "\n" } else { ", " });
                prev.push_str(&v);
            }
            None => {
                h.insert(k, v);
            }
        }
    }
    h
}

/// Undo chunked transfer coding; returns (body, trailers). Falls back to the input on bad framing.
pub fn dechunk(b: &[u8]) -> (Vec<u8>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let Some(nl) = b[at..].windows(2).position(|w| w == b"\r\n") else { return (b.to_vec(), Vec::new()) };
        let line = String::from_utf8_lossy(&b[at..at + nl]);
        let Ok(size) = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16) else { return (b.to_vec(), Vec::new()) };
        at += nl + 2;
        if size == 0 {
            let rest = String::from_utf8_lossy(&b[at..]);
            let tr = rest
                .lines()
                .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
                .collect();
            return (out, tr);
        }
        let Some(chunk) = b.get(at..at + size) else { return (b.to_vec(), Vec::new()) };
        out.extend_from_slice(chunk);
        at += size + 2;
    }
}

fn header_ci<'a>(h: &'a BTreeMap<String, String>, n: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
}

fn is_chunked(h: &BTreeMap<String, String>) -> bool {
    header_ci(h, "transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
}

fn head_lines(first: &str, h: &BTreeMap<String, String>) -> String {
    let mut s = format!("{first}\r\n");
    for (k, v) in h {
        let vals: Vec<&str> = if k.eq_ignore_ascii_case("set-cookie") { v.split('\n').collect() } else { vec![v.as_str()] };
        for v in vals {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    s.push_str("\r\n");
    s
}

fn body_for_wire(h: &BTreeMap<String, String>, body: &[u8], trailers: &[(String, String)]) -> Vec<u8> {
    if !is_chunked(h) {
        return body.to_vec();
    }
    let mut v = Vec::new();
    if !body.is_empty() {
        v.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
        v.extend_from_slice(body);
        v.extend_from_slice(b"\r\n");
    }
    v.extend_from_slice(b"0\r\n");
    for (k, val) in trailers {
        v.extend_from_slice(format!("{k}: {val}\r\n").as_bytes());
    }
    v.extend_from_slice(b"\r\n");
    v
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace(['\r', '\n'], " ")
}

fn xml_unesc(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Attribute `name="..."` of the first `<tag ...>` in `xml`.
fn attr(xml: &str, tag: &str, name: &str) -> Option<String> {
    let i = xml.find(&format!("<{tag} "))?;
    let el = &xml[i..i + xml[i..].find('>')?];
    let key = format!(" {name}=\"");
    let j = el.find(&key)? + key.len();
    Some(xml_unesc(&el[j..j + el[j..].find('"')?]))
}

/// SessionFlag N -> V pairs.
fn flags(xml: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for part in xml.split("<SessionFlag ").skip(1) {
        let el = format!("<SessionFlag {}", part);
        if let (Some(n), Some(v)) = (attr(&el, "SessionFlag", "N"), attr(&el, "SessionFlag", "V")) {
            out.insert(n.to_ascii_lowercase(), v);
        }
    }
    out
}

fn mark_color(m: SessionMark) -> Option<&'static str> {
    Some(match m {
        SessionMark::Red => "red",
        SessionMark::Blue => "blue",
        SessionMark::Green => "green",
        SessionMark::Orange => "orange",
        SessionMark::Purple => "purple",
        SessionMark::Copper => "#b87333",
        SessionMark::Strikeout => return None,
    })
}

fn color_mark(c: &str) -> Option<SessionMark> {
    Some(match c.to_ascii_lowercase().as_str() {
        "red" => SessionMark::Red,
        "blue" => SessionMark::Blue,
        "green" => SessionMark::Green,
        "orange" => SessionMark::Orange,
        "purple" => SessionMark::Purple,
        "#b87333" | "copper" => SessionMark::Copper,
        _ => return None,
    })
}

// ------------------------------------------------------------------------------------------ SAZ

/// The three files of one session.
pub fn session_files(n: usize, s: &SessionRecord) -> [(String, Vec<u8>); 3] {
    let id = format!("{n:0width$}", width = 2.max(1 + (n as f64).log10() as usize));
    let ver = if s.http_version.is_empty() { "HTTP/1.1" } else { &s.http_version };
    let mut c = head_lines(&format!("{} {} {}", s.method, s.url, ver), &s.request_headers).into_bytes();
    c.extend_from_slice(&body_for_wire(&s.request_headers, &s.request_bytes(), &[]));
    let rver = if s.response_http_version.is_empty() { ver } else { &s.response_http_version };
    let reason = crate::har::status_text(s.status);
    let mut r = head_lines(&format!("{rver} {} {reason}", s.status), &s.response_headers).into_bytes();
    r.extend_from_slice(&body_for_wire(&s.response_headers, &s.response_bytes(), &s.response_trailers));
    let begin = chrono::DateTime::parse_from_rfc3339(&s.start_time).ok();
    let done = begin.map(|b| b + chrono::Duration::milliseconds(s.duration_ms as i64));
    let ts = |t: Option<chrono::DateTime<chrono::FixedOffset>>| t.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Micros, false)).unwrap_or_default();
    let mut fl: Vec<(&str, String)> = Vec::new();
    if let Some(ip) = &s.client_ip {
        fl.push(("x-clientip", ip.clone()));
    }
    if let Some(ip) = &s.remote_ip {
        fl.push(("x-hostip", ip.clone()));
    }
    if let Some(p) = &s.process_name {
        fl.push(("x-processinfo", format!("{p}:{}", s.pid.unwrap_or(0))));
    }
    if !s.comment.is_empty() {
        fl.push(("ui-comments", s.comment.clone()));
    }
    if let Some(c) = s.mark.and_then(mark_color) {
        fl.push(("ui-color", c.to_string()));
    }
    if s.mark == Some(SessionMark::Strikeout) {
        fl.push(("ui-strikeout", "true".into()));
    }
    // Timers: ClientBeginRequest/ClientDoneResponse bound the session; the rest carry the waterfall
    // breakdown using Fiddler's own attribute names so it survives a round trip (and shows in Fiddler).
    let mut timers = format!("ClientBeginRequest=\"{}\" ClientDoneResponse=\"{}\"", ts(begin), ts(done));
    if let Some(d) = s.dns_ms {
        timers.push_str(&format!(" DNSTime=\"{d}\""));
    }
    if let Some(c) = s.connect_ms {
        timers.push_str(&format!(" TCPConnectTime=\"{c}\""));
    }
    if let Some(h) = s.tls_ms {
        timers.push_str(&format!(" HTTPSHandshakeTime=\"{h}\""));
    }
    if let (Some(b), Some(ttfb)) = (begin, s.ttfb_ms) {
        let pre = (s.dns_ms.unwrap_or(0) + s.connect_ms.unwrap_or(0) + s.tls_ms.unwrap_or(0)) as i64;
        let got = b + chrono::Duration::milliseconds(pre);
        let resp = got + chrono::Duration::milliseconds(ttfb as i64);
        timers.push_str(&format!(" ServerGotRequest=\"{}\" ServerBeginResponse=\"{}\"", ts(Some(got)), ts(Some(resp))));
    }
    let mut m = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Session SID=\"{n}\" BitFlags=\"0\">\r\n  <SessionTimers {timers} />\r\n  <SessionFlags>\r\n",
    );
    for (k, v) in fl {
        m.push_str(&format!("    <SessionFlag N=\"{k}\" V=\"{}\" />\r\n", xml_esc(&v)));
    }
    m.push_str("  </SessionFlags>\r\n</Session>");
    [(format!("raw/{id}_c.txt"), c), (format!("raw/{id}_s.txt"), r), (format!("raw/{id}_m.xml"), m.into_bytes())]
}

pub fn to_saz(sessions: &[SessionRecord]) -> Result<Vec<u8>, String> {
    let mut files = vec![(
        "[Content_Types].xml".to_string(),
        b"<?xml version=\"1.0\" encoding=\"utf-8\" ?>\r\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"htm\" ContentType=\"text/html\" /><Default Extension=\"xml\" ContentType=\"application/xml\" />\
<Default Extension=\"txt\" ContentType=\"text/plain\" /></Types>"
            .to_vec(),
    )];
    let mut index = String::from("<html><head><meta charset=\"utf-8\"><title>Sessions</title></head><body><table>\r\n<tr><th>#</th><th>Result</th><th>URL</th></tr>\r\n");
    for (i, s) in sessions.iter().enumerate() {
        index.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>\r\n", i + 1, s.status, xml_esc(&s.url)));
        files.extend(session_files(i + 1, s));
    }
    index.push_str("</table></body></html>");
    files.insert(1, ("_index.htm".into(), index.into_bytes()));
    zip_write(&files)
}

pub fn export_saz(path: &Path, sessions: &[SessionRecord]) -> Result<usize, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, to_saz(sessions)?).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(sessions.len())
}

/// One session from its `_c` / `_s` / `_m` files (`s` and `m` may be missing).
pub fn session_from_files(c: &[u8], s: Option<&[u8]>, m: Option<&str>) -> SessionRecord {
    let mut rec = SessionRecord::default();
    let (head, body) = split_head(c);
    let head = String::from_utf8_lossy(head);
    let mut lines = head.lines();
    let first: Vec<&str> = lines.next().unwrap_or("").split_whitespace().collect();
    rec.method = first.first().unwrap_or(&"GET").to_string();
    rec.http_version = first.get(2).unwrap_or(&"HTTP/1.1").to_string();
    rec.request_headers = parse_headers(lines);
    let target = first.get(1).copied().unwrap_or("/");
    rec.url = if target.contains("://") {
        target.to_string()
    } else if rec.method.eq_ignore_ascii_case("CONNECT") {
        format!("tunnel://{target}")
    } else {
        let host = header_ci(&rec.request_headers, "host").unwrap_or("unknown");
        let scheme = if host.ends_with(":80") { "http" } else { "https" };
        format!("{scheme}://{host}{target}")
    };
    let req_body = if is_chunked(&rec.request_headers) { dechunk(body).0 } else { body.to_vec() };
    rec.set_request_body(&req_body);
    if let Some(s) = s {
        let (head, body) = split_head(s);
        let head = String::from_utf8_lossy(head);
        let mut lines = head.lines();
        let first: Vec<&str> = lines.next().unwrap_or("").splitn(3, ' ').collect();
        rec.response_http_version = first.first().unwrap_or(&"").to_string();
        rec.status = first.get(1).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        rec.response_headers = parse_headers(lines);
        rec.response_raw_size = Some(body.len());
        let (b, tr) = if is_chunked(&rec.response_headers) { dechunk(body) } else { (body.to_vec(), Vec::new()) };
        rec.response_decoded_size = Some(b.len());
        rec.set_response_body(&b);
        rec.response_trailers = tr;
    }
    if let Some(m) = m {
        let begin = attr(m, "SessionTimers", "ClientBeginRequest").and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok());
        let done = attr(m, "SessionTimers", "ClientDoneResponse").and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok());
        if let Some(b) = begin {
            rec.start_time = b.to_rfc3339();
            if let Some(d) = done {
                rec.duration_ms = (d - b).num_milliseconds().max(0) as u128;
            }
        }
        rec.dns_ms = attr(m, "SessionTimers", "DNSTime").and_then(|v| v.trim().parse().ok());
        rec.connect_ms = attr(m, "SessionTimers", "TCPConnectTime").and_then(|v| v.trim().parse().ok());
        rec.tls_ms = attr(m, "SessionTimers", "HTTPSHandshakeTime").and_then(|v| v.trim().parse().ok());
        let got = attr(m, "SessionTimers", "ServerGotRequest").and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok());
        let server_resp = attr(m, "SessionTimers", "ServerBeginResponse").and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok());
        if let (Some(g), Some(r)) = (got, server_resp) {
            rec.ttfb_ms = Some((r - g).num_milliseconds().max(0) as u64);
        }
        let f = flags(m);
        rec.client_ip = f.get("x-clientip").cloned();
        rec.remote_ip = f.get("x-hostip").cloned();
        if let Some(p) = f.get("x-processinfo") {
            let (name, pid) = p.rsplit_once(':').unwrap_or((p, ""));
            rec.process_name = Some(name.to_string());
            rec.pid = pid.parse().ok().filter(|&p: &u32| p != 0);
        }
        rec.comment = f.get("ui-comments").cloned().unwrap_or_default();
        rec.mark = if f.get("ui-strikeout").is_some() { Some(SessionMark::Strikeout) } else { f.get("ui-color").and_then(|c| color_mark(c)) };
    }
    rec
}

pub fn parse_saz(data: &[u8]) -> Result<Vec<SessionRecord>, String> {
    let files = zip_read(data)?;
    let mut ids: Vec<(u64, String)> = files
        .keys()
        .filter_map(|k| {
            let stem = k.strip_prefix("raw/")?.strip_suffix("_c.txt")?;
            Some((stem.parse().ok()?, stem.to_string()))
        })
        .collect();
    if ids.is_empty() {
        return Err("no sessions in this SAZ (no raw/NN_c.txt entries)".into());
    }
    ids.sort();
    Ok(ids
        .iter()
        .map(|(_, id)| {
            let c = &files[&format!("raw/{id}_c.txt")];
            let s = files.get(&format!("raw/{id}_s.txt")).map(|v| v.as_slice());
            let m = files.get(&format!("raw/{id}_m.xml")).map(|v| String::from_utf8_lossy(v).into_owned());
            session_from_files(c, s, m.as_deref())
        })
        .collect())
}

pub fn import_saz(path: &Path) -> Result<Vec<SessionRecord>, String> {
    let data = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_saz(&data)
}

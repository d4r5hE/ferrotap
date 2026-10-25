//! G19 packet files: read pcap / pcapng, decode Ethernet / Linux SLL / raw IP, IPv4 / IPv6, TCP / UDP /
//! ICMP, and recognise DNS, TLS (with SNI) and HTTP. Statistics: conversations, endpoints, protocol
//! hierarchy, IO graph, DNS query log; follow a TCP stream; write a classic pcap file.
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::Path;

/// One captured frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Seconds since the epoch.
    pub ts: f64,
    pub linktype: u32,
    pub data: Vec<u8>,
    /// Length on the wire (may exceed `data` when the capture was truncated).
    pub orig_len: u32,
}

fn u16_at(b: &[u8], at: usize, be: bool) -> Option<u16> {
    let s: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
    Some(if be { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
}

fn u32_at(b: &[u8], at: usize, be: bool) -> Option<u32> {
    let s: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(if be { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
}

/// Read a pcap or pcapng file's frames.
pub fn parse(b: &[u8]) -> Result<Vec<Frame>, String> {
    let magic = u32_at(b, 0, false).ok_or("file too short")?;
    match magic {
        0xa1b2c3d4 | 0xa1b23c4d => parse_pcap(b, false, magic == 0xa1b23c4d),
        0xd4c3b2a1 | 0x4d3cb2a1 => parse_pcap(b, true, magic == 0x4d3cb2a1),
        0x0a0d0d0a => parse_pcapng(b),
        _ => Err("not a pcap or pcapng file".into()),
    }
}

fn parse_pcap(b: &[u8], be: bool, nanos: bool) -> Result<Vec<Frame>, String> {
    let linktype = u32_at(b, 20, be).ok_or("truncated pcap header")?;
    let mut out = Vec::new();
    let mut p = 24;
    while p + 16 <= b.len() {
        let sec = u32_at(b, p, be).unwrap_or(0) as f64;
        let frac = u32_at(b, p + 4, be).unwrap_or(0) as f64;
        let incl = u32_at(b, p + 8, be).unwrap_or(0) as usize;
        let orig = u32_at(b, p + 12, be).unwrap_or(0);
        let data = b.get(p + 16..p + 16 + incl).ok_or("truncated packet")?.to_vec();
        out.push(Frame { ts: sec + frac / if nanos { 1e9 } else { 1e6 }, linktype, data, orig_len: orig });
        p += 16 + incl;
    }
    Ok(out)
}

fn parse_pcapng(b: &[u8]) -> Result<Vec<Frame>, String> {
    let mut out = Vec::new();
    let mut be = false;
    // per interface: (linktype, ticks per second)
    let mut ifaces: Vec<(u32, f64)> = Vec::new();
    let mut p = 0;
    while p + 12 <= b.len() {
        let btype = u32_at(b, p, be).ok_or("truncated block")?;
        if btype == 0x0a0d0d0a {
            // section header: byte-order magic decides endianness
            be = u32_at(b, p + 8, false) == Some(0x4d3c2b1a);
            ifaces.clear();
        }
        let blen = u32_at(b, p + 4, be).ok_or("truncated block")? as usize;
        if blen < 12 || p + blen > b.len() {
            return Err(format!("bad block length {blen} at {p}"));
        }
        let body = &b[p + 8..p + blen - 4];
        match btype {
            1 => {
                // interface description: linktype, reserved, snaplen, options (if_tsresol = 9)
                let lt = u16_at(body, 0, be).unwrap_or(1) as u32;
                let mut res = 1e6;
                let mut q = 8;
                while q + 4 <= body.len() {
                    let code = u16_at(body, q, be).unwrap_or(0);
                    let len = u16_at(body, q + 2, be).unwrap_or(0) as usize;
                    if code == 0 {
                        break;
                    }
                    if code == 9 && len >= 1 {
                        let v = body[q + 4];
                        res = if v & 0x80 != 0 { 2f64.powi((v & 0x7f) as i32) } else { 10f64.powi(v as i32) };
                    }
                    q += 4 + len.div_ceil(4) * 4;
                }
                ifaces.push((lt, res));
            }
            6 => {
                // enhanced packet
                let iface = u32_at(body, 0, be).unwrap_or(0) as usize;
                let hi = u32_at(body, 4, be).unwrap_or(0) as u64;
                let lo = u32_at(body, 8, be).unwrap_or(0) as u64;
                let cap = u32_at(body, 12, be).unwrap_or(0) as usize;
                let orig = u32_at(body, 16, be).unwrap_or(0);
                let (lt, res) = ifaces.get(iface).copied().unwrap_or((1, 1e6));
                let data = body.get(20..20 + cap).ok_or("truncated packet")?.to_vec();
                out.push(Frame { ts: ((hi << 32) | lo) as f64 / res, linktype: lt, data, orig_len: orig });
            }
            3 => {
                // simple packet (no timestamp)
                let orig = u32_at(body, 0, be).unwrap_or(0);
                let (lt, _) = ifaces.first().copied().unwrap_or((1, 1e6));
                let n = (orig as usize).min(body.len().saturating_sub(4));
                out.push(Frame { ts: 0.0, linktype: lt, data: body[4..4 + n].to_vec(), orig_len: orig });
            }
            _ => {}
        }
        p += blen;
    }
    Ok(out)
}

/// A classic little-endian pcap (microseconds) of `frames` (all with the first frame's link type).
pub fn write_pcap(frames: &[&Frame]) -> Vec<u8> {
    let lt = frames.first().map_or(1, |f| f.linktype);
    let mut o = Vec::new();
    {
        let v = 0xa1b2c3d4u32;
        o.extend(v.to_le_bytes());
    }
    o.extend(2u16.to_le_bytes());
    o.extend(4u16.to_le_bytes());
    o.extend(0i32.to_le_bytes());
    o.extend(0u32.to_le_bytes());
    o.extend(262144u32.to_le_bytes());
    o.extend(lt.to_le_bytes());
    for f in frames {
        let sec = f.ts.floor();
        o.extend((sec as u32).to_le_bytes());
        o.extend((((f.ts - sec) * 1e6).round() as u32).min(999_999).to_le_bytes());
        o.extend((f.data.len() as u32).to_le_bytes());
        o.extend(f.orig_len.max(f.data.len() as u32).to_le_bytes());
        o.extend(&f.data);
    }
    o
}

pub fn read_file(p: &Path) -> Result<Vec<Frame>, String> {
    parse(&std::fs::read(p).map_err(|e| format!("read {}: {e}", p.display()))?)
}

// ------------------------------------------------------------------------------------------ decoding

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Packet {
    pub no: usize,
    /// Seconds since the first packet.
    pub time: f64,
    pub len: usize,
    pub src: Option<IpAddr>,
    pub dst: Option<IpAddr>,
    /// "TCP", "UDP", "ICMP", "ICMPv6", "ARP" or "" when not IP.
    pub transport: String,
    pub sport: u16,
    pub dport: u16,
    /// TCP flags byte.
    pub flags: u8,
    pub seq: u32,
    /// Application payload offset / length inside the frame data.
    pub payload_at: usize,
    pub payload_len: usize,
    /// Highest protocol recognised: "DNS", "TLS", "HTTP", or the transport.
    pub protocol: String,
    /// Protocol path for the hierarchy, e.g. ["eth", "ipv4", "tcp", "tls"].
    pub layers: Vec<&'static str>,
    pub info: String,
    pub dns: Option<DnsInfo>,
    pub sni: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DnsInfo {
    pub response: bool,
    pub name: String,
    pub qtype: u16,
    pub answers: Vec<String>,
}

pub fn qtype_name(t: u16) -> String {
    match t {
        1 => "A".into(),
        2 => "NS".into(),
        5 => "CNAME".into(),
        6 => "SOA".into(),
        12 => "PTR".into(),
        15 => "MX".into(),
        16 => "TXT".into(),
        28 => "AAAA".into(),
        33 => "SRV".into(),
        65 => "HTTPS".into(),
        n => format!("TYPE{n}"),
    }
}

fn dns_name(b: &[u8], mut p: usize, depth: usize) -> Option<(String, usize)> {
    let mut parts = Vec::new();
    let mut end = None;
    for _ in 0..128 {
        let l = *b.get(p)? as usize;
        if l == 0 {
            p += 1;
            break;
        }
        if l & 0xc0 == 0xc0 {
            let off = ((l & 0x3f) << 8) | *b.get(p + 1)? as usize;
            if depth > 8 {
                return None;
            }
            let (rest, _) = dns_name(b, off, depth + 1)?;
            if !rest.is_empty() {
                parts.push(rest);
            }
            end.get_or_insert(p + 2);
            return Some((parts.join("."), end.unwrap()));
        }
        parts.push(String::from_utf8_lossy(b.get(p + 1..p + 1 + l)?).into_owned());
        p += 1 + l;
    }
    Some((parts.join("."), end.unwrap_or(p)))
}

/// Decode a DNS message (question + A / AAAA / CNAME answers).
pub fn parse_dns(b: &[u8]) -> Option<DnsInfo> {
    if b.len() < 12 {
        return None;
    }
    let flags = u16_at(b, 2, true)?;
    let qd = u16_at(b, 4, true)?;
    let an = u16_at(b, 6, true)?;
    if qd == 0 {
        return None;
    }
    let (name, mut p) = dns_name(b, 12, 0)?;
    let qtype = u16_at(b, p, true)?;
    p += 4;
    let mut answers = Vec::new();
    for _ in 0..an.min(32) {
        let (_, q) = dns_name(b, p, 0)?;
        let t = u16_at(b, q, true)?;
        let rdlen = u16_at(b, q + 8, true)? as usize;
        let rd = b.get(q + 10..q + 10 + rdlen)?;
        match (t, rdlen) {
            (1, 4) => answers.push(format!("{}.{}.{}.{}", rd[0], rd[1], rd[2], rd[3])),
            (28, 16) => answers.push(std::net::Ipv6Addr::from(<[u8; 16]>::try_from(rd).ok()?).to_string()),
            (5, _) => answers.push(format!("CNAME {}", dns_name(b, q + 10, 0).map(|x| x.0).unwrap_or_default())),
            _ => answers.push(qtype_name(t)),
        }
        p = q + 10 + rdlen;
    }
    Some(DnsInfo { response: flags & 0x8000 != 0, name, qtype, answers })
}

/// Server name from a TLS ClientHello (None for anything else).
pub fn tls_sni(b: &[u8]) -> Option<String> {
    if b.first() != Some(&0x16) || b.get(5) != Some(&0x01) {
        return None;
    }
    let mut p = 9 + 2 + 32; // record header 5 + handshake header 4, version, random
    let sid = *b.get(p)? as usize;
    p += 1 + sid;
    let cs = u16_at(b, p, true)? as usize;
    p += 2 + cs;
    let cm = *b.get(p)? as usize;
    p += 1 + cm;
    let ext_end = p + 2 + u16_at(b, p, true)? as usize;
    p += 2;
    while p + 4 <= ext_end.min(b.len()) {
        let t = u16_at(b, p, true)?;
        let l = u16_at(b, p + 2, true)? as usize;
        if t == 0 {
            let nl = u16_at(b, p + 7, true)? as usize;
            return Some(String::from_utf8_lossy(b.get(p + 9..p + 9 + nl)?).into_owned());
        }
        p += 4 + l;
    }
    None
}

const HTTP_STARTS: &[&[u8]] = &[b"GET ", b"POST ", b"PUT ", b"DELETE ", b"HEAD ", b"OPTIONS ", b"PATCH ", b"CONNECT ", b"HTTP/1."];

fn decode_ip(frame: &[u8], at: usize, p: &mut Packet) {
    let Some(&v) = frame.get(at) else { return };
    let (proto, l4) = match v >> 4 {
        4 => {
            let ihl = (v & 0x0f) as usize * 4;
            let Some(h) = frame.get(at..at + 20) else { return };
            p.src = Some(IpAddr::from([h[12], h[13], h[14], h[15]]));
            p.dst = Some(IpAddr::from([h[16], h[17], h[18], h[19]]));
            p.layers.push("ipv4");
            (h[9], at + ihl)
        }
        6 => {
            let Some(h) = frame.get(at..at + 40) else { return };
            p.src = Some(IpAddr::from(<[u8; 16]>::try_from(&h[8..24]).unwrap()));
            p.dst = Some(IpAddr::from(<[u8; 16]>::try_from(&h[24..40]).unwrap()));
            p.layers.push("ipv6");
            (h[6], at + 40)
        }
        _ => return,
    };
    match proto {
        6 => {
            let Some(h) = frame.get(l4..l4 + 20) else { return };
            p.transport = "TCP".into();
            p.layers.push("tcp");
            p.sport = u16::from_be_bytes([h[0], h[1]]);
            p.dport = u16::from_be_bytes([h[2], h[3]]);
            p.seq = u32::from_be_bytes([h[4], h[5], h[6], h[7]]);
            p.flags = h[13];
            p.payload_at = l4 + ((h[12] >> 4) as usize) * 4;
        }
        17 => {
            let Some(h) = frame.get(l4..l4 + 8) else { return };
            p.transport = "UDP".into();
            p.layers.push("udp");
            p.sport = u16::from_be_bytes([h[0], h[1]]);
            p.dport = u16::from_be_bytes([h[2], h[3]]);
            p.payload_at = l4 + 8;
        }
        1 => {
            p.transport = "ICMP".into();
            p.layers.push("icmp");
            p.payload_at = frame.len();
        }
        58 => {
            p.transport = "ICMPv6".into();
            p.layers.push("icmpv6");
            p.payload_at = frame.len();
        }
        _ => {
            p.transport = format!("IP proto {proto}");
            p.payload_at = frame.len();
        }
    }
    p.payload_len = frame.len().saturating_sub(p.payload_at);
}

fn tcp_flags_text(f: u8) -> String {
    let names = [(0x02, "SYN"), (0x10, "ACK"), (0x01, "FIN"), (0x04, "RST"), (0x08, "PSH")];
    names.iter().filter(|(b, _)| f & b != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(", ")
}

/// Decode every frame (times relative to the first).
pub fn decode_all(frames: &[Frame]) -> Vec<Packet> {
    let t0 = frames.iter().map(|f| f.ts).find(|t| *t > 0.0).unwrap_or(0.0);
    frames.iter().enumerate().map(|(i, f)| decode(i + 1, f, t0)).collect()
}

pub fn decode(no: usize, f: &Frame, t0: f64) -> Packet {
    let d = &f.data;
    let mut p = Packet { no, time: (f.ts - t0).max(0.0), len: f.orig_len.max(d.len() as u32) as usize, ..Default::default() };
    match f.linktype {
        1 => {
            p.layers.push("eth");
            let mut et = u16_at(d, 12, true).unwrap_or(0);
            let mut at = 14;
            if et == 0x8100 {
                et = u16_at(d, 16, true).unwrap_or(0);
                at = 18;
            }
            match et {
                0x0800 | 0x86dd => decode_ip(d, at, &mut p),
                0x0806 => {
                    p.transport = "ARP".into();
                    p.layers.push("arp");
                }
                _ => {}
            }
        }
        113 => {
            p.layers.push("sll");
            decode_ip(d, 16, &mut p);
        }
        101 | 228 | 229 | 12 | 14 => decode_ip(d, 0, &mut p),
        _ => {}
    }
    let payload = d.get(p.payload_at..p.payload_at + p.payload_len).unwrap_or(&[]);
    p.protocol = p.transport.clone();
    if (p.transport == "UDP" || p.transport == "TCP") && (p.sport == 53 || p.dport == 53 || p.sport == 5353 || p.dport == 5353) {
        let msg = if p.transport == "TCP" { payload.get(2..).unwrap_or(&[]) } else { payload };
        if let Some(dns) = parse_dns(msg) {
            p.protocol = "DNS".into();
            p.layers.push("dns");
            p.info = format!("{} {} {}{}", if dns.response { "response" } else { "query" }, qtype_name(dns.qtype), dns.name, if dns.answers.is_empty() { String::new() } else { format!(" -> {}", dns.answers.join(", ")) });
            p.dns = Some(dns);
        }
    } else if p.transport == "TCP" && !payload.is_empty() {
        if HTTP_STARTS.iter().any(|s| payload.starts_with(s)) {
            p.protocol = "HTTP".into();
            p.layers.push("http");
            p.info = String::from_utf8_lossy(payload.split(|&c| c == b'\r' || c == b'\n').next().unwrap_or(&[])).chars().take(120).collect();
        } else if payload.len() >= 5 && (0x14..=0x17).contains(&payload[0]) && payload[1] == 3 {
            p.protocol = "TLS".into();
            p.layers.push("tls");
            p.sni = tls_sni(payload);
            p.info = match (&p.sni, payload[0]) {
                (Some(s), _) => format!("Client Hello (SNI {s})"),
                (None, 0x16) => "Handshake".into(),
                (None, 0x17) => "Application Data".into(),
                (None, 0x15) => "Alert".into(),
                _ => "Change Cipher Spec".into(),
            };
        }
    }
    if p.info.is_empty() {
        p.info = match p.transport.as_str() {
            "TCP" => format!("{} -> {} [{}] len={}", p.sport, p.dport, tcp_flags_text(p.flags), p.payload_len),
            "UDP" => format!("{} -> {} len={}", p.sport, p.dport, p.payload_len),
            t => t.to_string(),
        };
    }
    p
}

// ------------------------------------------------------------------------------------------ statistics

/// Conversation key: (lower endpoint, higher endpoint, transport) so both directions match.
pub type ConvKey = (String, String, String);

pub fn endpoint(ip: Option<IpAddr>, port: u16, transport: &str) -> String {
    match ip {
        Some(IpAddr::V6(a)) if transport == "TCP" || transport == "UDP" => format!("[{a}]:{port}"),
        Some(a) if transport == "TCP" || transport == "UDP" => format!("{a}:{port}"),
        Some(a) => a.to_string(),
        None => "-".into(),
    }
}

pub fn conv_key(p: &Packet) -> Option<ConvKey> {
    p.src?;
    let a = endpoint(p.src, p.sport, &p.transport);
    let b = endpoint(p.dst, p.dport, &p.transport);
    Some(if a <= b { (a, b, p.transport.clone()) } else { (b, a, p.transport.clone()) })
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Conversation {
    pub a: String,
    pub b: String,
    pub transport: String,
    pub packets: usize,
    pub bytes: usize,
    pub start: f64,
    pub end: f64,
}

pub fn conversations(pk: &[&Packet]) -> Vec<Conversation> {
    let mut m: BTreeMap<ConvKey, Conversation> = BTreeMap::new();
    for p in pk {
        let Some(k) = conv_key(p) else { continue };
        let c = m.entry(k.clone()).or_insert_with(|| Conversation { a: k.0.clone(), b: k.1.clone(), transport: k.2.clone(), start: p.time, ..Default::default() });
        c.packets += 1;
        c.bytes += p.len;
        c.start = c.start.min(p.time);
        c.end = c.end.max(p.time);
    }
    let mut v: Vec<Conversation> = m.into_values().collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    v
}

/// (address, packets, bytes) per IP address, busiest first.
pub fn endpoints(pk: &[&Packet]) -> Vec<(String, usize, usize)> {
    let mut m: HashMap<String, (usize, usize)> = HashMap::new();
    for p in pk {
        for ip in [p.src, p.dst].into_iter().flatten() {
            let e = m.entry(ip.to_string()).or_default();
            e.0 += 1;
            e.1 += p.len;
        }
    }
    let mut v: Vec<(String, usize, usize)> = m.into_iter().map(|(k, (a, b))| (k, a, b)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    v
}

/// Protocol hierarchy: ("eth/ipv4/tcp/tls", packets, bytes) for every path prefix, in path order.
pub fn hierarchy(pk: &[&Packet]) -> Vec<(String, usize, usize)> {
    let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for p in pk {
        for i in 1..=p.layers.len() {
            let e = m.entry(p.layers[..i].join("/")).or_default();
            e.0 += 1;
            e.1 += p.len;
        }
    }
    m.into_iter().map(|(k, (a, b))| (k, a, b)).collect()
}

/// IO graph: (packets, bytes) per `interval` seconds from 0.
pub fn io_graph(pk: &[&Packet], interval: f64) -> Vec<(usize, usize)> {
    let interval = interval.max(0.001);
    let n = pk.iter().map(|p| (p.time / interval) as usize).max().map_or(0, |m| m + 1).min(100_000);
    let mut v = vec![(0usize, 0usize); n];
    for p in pk {
        let i = ((p.time / interval) as usize).min(n.saturating_sub(1));
        if let Some(x) = v.get_mut(i) {
            x.0 += 1;
            x.1 += p.len;
        }
    }
    v
}

/// DNS log rows: (time, query or response, type, name, answers).
pub fn dns_log(pk: &[&Packet]) -> Vec<(f64, bool, String, String, String)> {
    pk.iter().filter_map(|p| p.dns.as_ref().map(|d| (p.time, d.response, qtype_name(d.qtype), d.name.clone(), d.answers.join(", ")))).collect()
}

/// Reassembled payload of the TCP conversation of `pkt`: (client -> server bytes, server -> client
/// bytes, chunks in order as (from_client, text)). Retransmitted segments are dropped.
pub fn follow_tcp(frames: &[Frame], pk: &[Packet], pkt: &Packet) -> (usize, usize, Vec<(bool, Vec<u8>)>) {
    let Some(key) = conv_key(pkt) else { return (0, 0, Vec::new()) };
    let client = pk.iter().find(|p| conv_key(p).as_ref() == Some(&key)).map(|p| endpoint(p.src, p.sport, &p.transport)).unwrap_or_default();
    let mut seen: HashMap<(bool, u32), ()> = HashMap::new();
    let mut chunks: Vec<(bool, Vec<u8>)> = Vec::new();
    let (mut up, mut down) = (0, 0);
    for p in pk.iter().filter(|p| p.transport == "TCP" && conv_key(p).as_ref() == Some(&key) && p.payload_len > 0) {
        let from_client = endpoint(p.src, p.sport, &p.transport) == client;
        if seen.insert((from_client, p.seq), ()).is_some() {
            continue;
        }
        let data = frames[p.no - 1].data.get(p.payload_at..p.payload_at + p.payload_len).unwrap_or(&[]).to_vec();
        if from_client { up += data.len() } else { down += data.len() }
        match chunks.last_mut() {
            Some((d, buf)) if *d == from_client => buf.extend(data),
            _ => chunks.push((from_client, data)),
        }
    }
    (up, down, chunks)
}

/// Captured HTTP sessions that belong to a conversation: same server address, overlapping in time.
/// `t0` is the capture's first timestamp (seconds since the epoch).
pub fn related_sessions(conv: &Conversation, sessions: &[crate::model::SessionRecord], t0: f64) -> Vec<usize> {
    let ips: Vec<String> = [&conv.a, &conv.b].iter().map(|e| e.trim_start_matches('[').rsplit_once(':').map(|(h, _)| h.trim_end_matches(']').to_string()).unwrap_or_else(|| e.to_string())).collect();
    sessions
        .iter()
        .filter(|s| s.remote_ip.as_ref().is_some_and(|ip| ips.contains(ip)))
        .filter(|s| {
            chrono::DateTime::parse_from_rfc3339(&s.start_time).map_or(true, |st| {
                let t = st.timestamp_millis() as f64 / 1000.0 - t0;
                t >= conv.start - 5.0 && t <= conv.end + 5.0
            })
        })
        .map(|s| s.id)
        .collect()
}

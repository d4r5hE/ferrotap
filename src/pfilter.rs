//! G19 display filters for packets, in the familiar style:
//!   tcp.port == 443 and not ip.addr == 10.0.0.1
//!   dns.qry.name contains "example" || http
//!   frame.len > 1000 && (tcp.flags.syn == 1 || udp)
//! Bare protocol names (tcp, udp, dns, http, tls, icmp, arp, ipv4, ipv6) test for the layer.
use crate::pcap::Packet;

/// Field names with a short description (help and completion).
pub const FIELDS: &[(&str, &str)] = &[
    ("frame.number", "packet number"),
    ("frame.len", "length on the wire"),
    ("frame.time_relative", "seconds since the first packet"),
    ("ip.addr", "source or destination address"),
    ("ip.src", "source address"),
    ("ip.dst", "destination address"),
    ("tcp.port", "source or destination TCP port"),
    ("tcp.srcport", "TCP source port"),
    ("tcp.dstport", "TCP destination port"),
    ("tcp.len", "TCP payload length"),
    ("tcp.seq", "TCP sequence number"),
    ("tcp.flags.syn", "1 when SYN is set"),
    ("tcp.flags.ack", "1 when ACK is set"),
    ("tcp.flags.fin", "1 when FIN is set"),
    ("tcp.flags.rst", "1 when RST is set"),
    ("udp.port", "source or destination UDP port"),
    ("udp.srcport", "UDP source port"),
    ("udp.dstport", "UDP destination port"),
    ("dns.qry.name", "DNS query name"),
    ("dns.qry.type", "DNS query type (numeric, e.g. 1 for A)"),
    ("dns.a", "DNS A-record answer address"),
    ("dns.flags.response", "1 for DNS responses"),
    ("tls.sni", "TLS server name (Client Hello)"),
    ("http.request.line", "first line of an HTTP message"),
    ("http.request.method", "HTTP request method (GET, POST, …)"),
    ("http.request.uri", "HTTP request target (path)"),
    ("http.response.code", "HTTP response status code"),
    ("info", "the Info column text"),
];

pub const PROTOCOLS: &[&str] = &["tcp", "udp", "dns", "http", "tls", "icmp", "icmpv6", "arp", "ipv4", "ipv6", "eth"];

pub const HELP: &str = "Compare: ==  !=  >  <  >=  <=  contains  matches (regex)\n\
Membership: field in { a b c }  (sets; numeric ranges like {1..1024})\n\
Combine: and / &&,  or / ||,  not / !,  ( )\n\
Values: numbers, addresses, \"quoted text\"\n\
Examples: tcp.port == 443   ip.addr == 10.0.0.0/24 && !dns   tcp.port in {80 443 8080}   frame.len > 1000";

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Op(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() || ch == ',' {
            // Commas only separate set members; treat them like spaces.
            i += 1;
        } else if ch == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if ch == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else if ch == '{' {
            out.push(Tok::LBrace);
            i += 1;
        } else if ch == '}' {
            out.push(Tok::RBrace);
            i += 1;
        } else if ch == '"' {
            let mut v = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                }
                v.push(c[i]);
                i += 1;
            }
            if i >= c.len() {
                return Err("unclosed quote".into());
            }
            i += 1;
            out.push(Tok::Str(v));
        } else if "=!<>&|".contains(ch) {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let op = if ["==", "!=", ">=", "<=", "&&", "||"].contains(&two.as_str()) {
                two
            } else {
                ch.to_string()
            };
            if op == "=" || op == "&" || op == "|" {
                return Err(format!("unknown operator '{op}' (use == && ||)"));
            }
            i += op.len();
            out.push(Tok::Op(op));
        } else {
            let mut v = String::new();
            while i < c.len() && !c[i].is_whitespace() && c[i] != ',' && !"()\"=!<>&|{}".contains(c[i]) {
                v.push(c[i]);
                i += 1;
            }
            out.push(Tok::Word(v));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    /// Field exists / protocol present.
    Has(String),
    Cmp(String, String, String),
    /// `field in { a b c }` — any field value equals a member (or falls in a numeric range).
    In(String, Vec<String>),
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn word_is(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
    }
    fn op_is(&self, o: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(x)) if x == o)
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.word_is("or") || self.op_is("||") {
            self.i += 1;
            l = Expr::Or(Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while self.word_is("and") || self.op_is("&&") {
            self.i += 1;
            l = Expr::And(Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }
    fn not(&mut self) -> Result<Expr, String> {
        if self.word_is("not") || self.op_is("!") {
            self.i += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.primary()
    }
    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Tok::LParen) => {
                self.i += 1;
                let e = self.or()?;
                if self.peek() != Some(&Tok::RParen) {
                    return Err("missing ')'".into());
                }
                self.i += 1;
                Ok(e)
            }
            Some(Tok::Word(f)) => {
                self.i += 1;
                let f = f.to_ascii_lowercase();
                let known = PROTOCOLS.contains(&f.as_str()) || FIELDS.iter().any(|(n, _)| *n == f) || f == "tls.handshake.extensions_server_name";
                if !known {
                    return Err(format!("unknown field '{f}'"));
                }
                if self.word_is("in") {
                    self.i += 1;
                    if self.peek() != Some(&Tok::LBrace) {
                        return Err(format!("'{f} in' needs a {{ set }}"));
                    }
                    self.i += 1;
                    let mut items = Vec::new();
                    loop {
                        match self.peek().cloned() {
                            Some(Tok::Word(v)) | Some(Tok::Str(v)) => {
                                items.push(v);
                                self.i += 1;
                            }
                            Some(Tok::RBrace) => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(format!("unclosed set in '{f} in {{ ... }}'")),
                        }
                    }
                    if items.is_empty() {
                        return Err(format!("'{f} in {{}}' has no members"));
                    }
                    return Ok(Expr::In(f, items));
                }
                let op = match self.peek() {
                    Some(Tok::Op(o)) if ["==", "!=", ">", "<", ">=", "<="].contains(&o.as_str()) => o.clone(),
                    Some(Tok::Word(w)) if ["eq", "ne", "gt", "lt", "ge", "le", "contains", "matches"].contains(&w.to_ascii_lowercase().as_str()) => {
                        match w.to_ascii_lowercase().as_str() {
                            "eq" => "==".into(),
                            "ne" => "!=".into(),
                            "gt" => ">".into(),
                            "lt" => "<".into(),
                            "ge" => ">=".into(),
                            "le" => "<=".into(),
                            other => other.to_string(),
                        }
                    }
                    _ => return Ok(Expr::Has(f)),
                };
                self.i += 1;
                let v = match self.peek().cloned() {
                    Some(Tok::Word(v)) | Some(Tok::Str(v)) => v,
                    _ => return Err(format!("'{f} {op}' needs a value")),
                };
                self.i += 1;
                if op == "matches" {
                    regex::Regex::new(&v).map_err(|e| format!("bad regex: {e}"))?;
                }
                Ok(Expr::Cmp(f, op, v))
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("the filter ends too early".into()),
        }
    }
}

/// Parse a filter; "" is "everything".
pub fn parse(s: &str) -> Result<Option<Expr>, String> {
    if s.trim().is_empty() {
        return Ok(None);
    }
    let mut p = P { t: lex(s)?, i: 0 };
    let e = p.or()?;
    if p.i < p.t.len() {
        return Err(format!("unexpected {:?} after the filter", p.t[p.i]));
    }
    Ok(Some(e))
}

#[derive(Debug, Clone)]
enum Val {
    N(f64),
    S(String),
}

fn values(f: &str, p: &Packet) -> Vec<Val> {
    let n = |x: f64| vec![Val::N(x)];
    let tcp = p.transport == "TCP";
    let udp = p.transport == "UDP";
    match f {
        "frame.number" => n(p.no as f64),
        "frame.len" => n(p.len as f64),
        "frame.time_relative" => n(p.time),
        "ip.addr" => [p.src, p.dst].iter().flatten().map(|a| Val::S(a.to_string())).collect(),
        "ip.src" => p.src.iter().map(|a| Val::S(a.to_string())).collect(),
        "ip.dst" => p.dst.iter().map(|a| Val::S(a.to_string())).collect(),
        "tcp.port" if tcp => vec![Val::N(p.sport as f64), Val::N(p.dport as f64)],
        "tcp.srcport" if tcp => n(p.sport as f64),
        "tcp.dstport" if tcp => n(p.dport as f64),
        "tcp.len" if tcp => n(p.payload_len as f64),
        "tcp.seq" if tcp => n(p.seq as f64),
        "tcp.flags.syn" if tcp => n(((p.flags & 0x02) != 0) as u8 as f64),
        "tcp.flags.ack" if tcp => n(((p.flags & 0x10) != 0) as u8 as f64),
        "tcp.flags.fin" if tcp => n(((p.flags & 0x01) != 0) as u8 as f64),
        "tcp.flags.rst" if tcp => n(((p.flags & 0x04) != 0) as u8 as f64),
        "udp.port" if udp => vec![Val::N(p.sport as f64), Val::N(p.dport as f64)],
        "udp.srcport" if udp => n(p.sport as f64),
        "udp.dstport" if udp => n(p.dport as f64),
        "dns.qry.name" => p.dns.iter().map(|d| Val::S(d.name.clone())).collect(),
        "dns.qry.type" => p.dns.iter().map(|d| Val::N(d.qtype as f64)).collect(),
        "dns.a" => p.dns.iter().flat_map(|d| d.answers.iter().map(|a| Val::S(a.clone()))).collect(),
        "dns.flags.response" => p.dns.iter().map(|d| Val::N(d.response as u8 as f64)).collect(),
        "tls.sni" | "tls.handshake.extensions_server_name" => p.sni.iter().map(|s| Val::S(s.clone())).collect(),
        "http.request.line" if p.protocol == "HTTP" => vec![Val::S(p.info.clone())],
        // The HTTP Info line is either a request line ("GET /p HTTP/1.1") or a status line
        // ("HTTP/1.1 200 OK"); split accordingly.
        "http.request.method" if p.protocol == "HTTP" && !p.info.starts_with("HTTP/") => {
            p.info.split_whitespace().next().map(|m| Val::S(m.to_string())).into_iter().collect()
        }
        "http.request.uri" if p.protocol == "HTTP" && !p.info.starts_with("HTTP/") => {
            p.info.split_whitespace().nth(1).map(|u| Val::S(u.to_string())).into_iter().collect()
        }
        "http.response.code" if p.protocol == "HTTP" && p.info.starts_with("HTTP/") => {
            p.info.split_whitespace().nth(1).and_then(|c| c.parse::<f64>().ok()).map(Val::N).into_iter().collect()
        }
        "info" => vec![Val::S(p.info.clone())],
        _ => Vec::new(),
    }
}

/// Is `ip` inside the `addr/prefix` network? `None` when `cidr` is not a valid CIDR.
fn ip_in_cidr(ip: std::net::IpAddr, cidr: &str) -> Option<bool> {
    let (net_s, prefix_s) = cidr.split_once('/')?;
    let net: std::net::IpAddr = net_s.trim().parse().ok()?;
    let prefix: u32 = prefix_s.trim().parse().ok()?;
    match (ip, net) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n)) => {
            if prefix > 32 {
                return None;
            }
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            Some(u32::from(a) & mask == u32::from(n) & mask)
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(n)) => {
            if prefix > 128 {
                return None;
            }
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            Some(u128::from(a) & mask == u128::from(n) & mask)
        }
        _ => Some(false), // an IPv4 address is never inside an IPv6 network, or vice versa
    }
}

/// A single `in { … }` member against one field value: a numeric range `lo..hi`
/// (inclusive) for numeric values, otherwise plain equality (which covers CIDR).
fn in_match(v: &Val, want: &str) -> bool {
    if let Some((lo, hi)) = want.split_once("..") {
        if let (Ok(lo), Ok(hi)) = (lo.trim().parse::<f64>(), hi.trim().parse::<f64>()) {
            return matches!(v, Val::N(x) if *x >= lo && *x <= hi);
        }
    }
    cmp(v, "==", want)
}

fn cmp(v: &Val, op: &str, want: &str) -> bool {
    // Subnet membership: ip.addr == 10.0.0.0/24 (and != for "not in this subnet").
    if (op == "==" || op == "!=") && want.contains('/') {
        if let Val::S(s) = v {
            if let Ok(ip) = s.parse::<std::net::IpAddr>() {
                if let Some(inside) = ip_in_cidr(ip, want) {
                    return if op == "==" { inside } else { !inside };
                }
            }
        }
    }
    match op {
        "contains" => match v {
            Val::S(s) => s.to_ascii_lowercase().contains(&want.to_ascii_lowercase()),
            Val::N(x) => x.to_string().contains(want),
        },
        "matches" => {
            let s = match v {
                Val::S(s) => s.clone(),
                Val::N(x) => x.to_string(),
            };
            regex::Regex::new(want).is_ok_and(|r| r.is_match(&s))
        }
        _ => {
            let ord = match (v, want.parse::<f64>()) {
                (Val::N(x), Ok(w)) => x.partial_cmp(&w),
                (Val::N(x), Err(_)) => x.to_string().partial_cmp(&want.to_string()),
                (Val::S(s), _) => {
                    // addresses compare as parsed IPs (so ::1 == 0:0::1)
                    match (s.parse::<std::net::IpAddr>(), want.parse::<std::net::IpAddr>()) {
                        (Ok(a), Ok(b)) => Some(a.cmp(&b)),
                        _ => Some(s.to_ascii_lowercase().cmp(&want.to_ascii_lowercase())),
                    }
                }
            };
            let Some(o) = ord else { return false };
            match op {
                "==" => o.is_eq(),
                "!=" => o.is_ne(),
                ">" => o.is_gt(),
                "<" => o.is_lt(),
                ">=" => o.is_ge(),
                "<=" => o.is_le(),
                _ => false,
            }
        }
    }
}

pub fn eval(e: &Expr, p: &Packet) -> bool {
    match e {
        Expr::And(a, b) => eval(a, p) && eval(b, p),
        Expr::Or(a, b) => eval(a, p) || eval(b, p),
        Expr::Not(a) => !eval(a, p),
        Expr::Has(f) => {
            if PROTOCOLS.contains(&f.as_str()) {
                p.layers.contains(&f.as_str())
            } else {
                !values(f, p).is_empty()
            }
        }
        Expr::Cmp(f, op, v) => {
            let vals = values(f, p);
            if op == "!=" {
                !vals.is_empty() && vals.iter().all(|x| cmp(x, "!=", v))
            } else {
                vals.iter().any(|x| cmp(x, op, v))
            }
        }
        Expr::In(f, items) => {
            let vals = values(f, p);
            vals.iter().any(|x| items.iter().any(|w| in_match(x, w)))
        }
    }
}

/// Field / protocol names starting with the last word of `typed` (for completion).
pub fn complete(typed: &str) -> Vec<&'static str> {
    let last = typed.rsplit(|c: char| c.is_whitespace() || "()!&|".contains(c)).next().unwrap_or("").to_ascii_lowercase();
    if last.is_empty() {
        return Vec::new();
    }
    PROTOCOLS.iter().copied().chain(FIELDS.iter().map(|(n, _)| *n)).filter(|n| n.starts_with(&last) && *n != last).take(10).collect()
}

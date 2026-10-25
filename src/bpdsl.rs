//! mitmproxy-style filter DSL for breakpoints. Pure and side-effect free: parse text -> `Expr`,
//! then `Expr::eval(&Ctx)` -> bool. Used by `BreakpointRule` when `match_kind == "dsl"`.
//!
//! Grammar (precedence: `!` > `&` > `|`):
//!   ~all          anything          ~q  request stage       ~s  response stage
//!   ~u <regex>    URL               ~d <regex>  host        ~m <method>  exact, case-insensitive
//!   ~h <regex>    header, matched as "name: value" (current stage's headers)
//!   ~t <regex>    Content-Type of the current stage
//!   ~b <regex>    body of the current stage (decoded response body at the response stage)
//!   ~c <spec>     status: 404, 5xx, 400-499, 401,403,5xx (never true at request stage)
//!   ( expr )      ! expr      a & b      a | b
//! Arguments with spaces or operators must be quoted: ~u "a|b". Regexes are case-insensitive.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub status: u16,
    pub response: bool,
    /// Headers for the current stage: request headers at the request stage, response headers at
    /// the response stage. Empty when the caller has none (e.g. a UI match preview).
    pub headers: &'a [(String, String)],
    /// Body for the current stage (request body at the request stage, decoded response body at the
    /// response stage). Empty when unavailable.
    pub body: &'a str,
}

#[derive(Debug)]
pub enum Expr {
    All,
    Req,
    Resp,
    Url(Regex),
    Domain(Regex),
    Method(String),
    Status(String),
    /// `~h <regex>` — any current-stage header, matched as "name: value".
    Header(Regex),
    /// `~t <regex>` — the current stage's Content-Type header value.
    ContentType(Regex),
    /// `~b <regex>` — the current stage's body.
    Body(Regex),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|x| x.1).unwrap_or(url);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let auth = auth.rsplit_once('@').map(|x| x.1).unwrap_or(auth);
    if auth.starts_with('[') {
        return auth.split(']').next().unwrap_or(auth).trim_start_matches('[');
    }
    auth.split(':').next().unwrap_or(auth)
}

impl Expr {
    /// Does this filter inspect the body? The proxy uses this to decide whether a response
    /// breakpoint needs the (decoded) body buffered before it can be evaluated.
    pub fn uses_body(&self) -> bool {
        match self {
            Expr::Body(_) => true,
            Expr::Not(e) => e.uses_body(),
            Expr::And(a, b) | Expr::Or(a, b) => a.uses_body() || b.uses_body(),
            _ => false,
        }
    }

    /// Does this filter inspect headers (`~h`/`~t`)? Lets a caller skip building a header list
    /// when it isn't needed.
    pub fn uses_headers(&self) -> bool {
        match self {
            Expr::Header(_) | Expr::ContentType(_) => true,
            Expr::Not(e) => e.uses_headers(),
            Expr::And(a, b) | Expr::Or(a, b) => a.uses_headers() || b.uses_headers(),
            _ => false,
        }
    }

    /// Evaluate against a completed flow (e.g. the session grid) rather than one live stage: a
    /// flow always has a request (`~q`), has a response (`~s`) when `status != 0`, and `~h`/`~b`/
    /// `~t` are expected to see the request and response headers/body merged into `c`. All other
    /// terms behave as in `eval`.
    pub fn eval_flow(&self, c: &Ctx) -> bool {
        match self {
            Expr::Req => true,
            Expr::Resp => c.status != 0,
            Expr::Status(s) => c.status != 0 && crate::model::bp_status_matches(s, c.status),
            Expr::Not(e) => !e.eval_flow(c),
            Expr::And(a, b) => a.eval_flow(c) && b.eval_flow(c),
            Expr::Or(a, b) => a.eval_flow(c) || b.eval_flow(c),
            // All, Url, Domain, Method, Header, ContentType, Body are stage-independent.
            other => other.eval(c),
        }
    }

    pub fn eval(&self, c: &Ctx) -> bool {
        match self {
            Expr::All => true,
            Expr::Req => !c.response,
            Expr::Resp => c.response,
            Expr::Url(r) => r.is_match(c.url),
            Expr::Domain(r) => r.is_match(host_of(c.url)),
            Expr::Method(m) => m.eq_ignore_ascii_case(c.method),
            Expr::Status(s) => c.response && crate::model::bp_status_matches(s, c.status),
            Expr::Header(r) => c.headers.iter().any(|(k, v)| r.is_match(&format!("{k}: {v}"))),
            Expr::ContentType(r) => c.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && r.is_match(v)),
            Expr::Body(r) => r.is_match(c.body),
            Expr::Not(e) => !e.eval(c),
            Expr::And(a, b) => a.eval(c) && b.eval(c),
            Expr::Or(a, b) => a.eval(c) || b.eval(c),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LP,
    RP,
    And,
    Or,
    Not,
    Flag(String),
    Word(String),
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            out.push(Tok::LP);
            i += 1;
        } else if c == ')' {
            out.push(Tok::RP);
            i += 1;
        } else if c == '&' {
            out.push(Tok::And);
            i += 1;
        } else if c == '|' {
            out.push(Tok::Or);
            i += 1;
        } else if c == '!' {
            out.push(Tok::Not);
            i += 1;
        } else if c == '"' || c == '\'' {
            let q = c;
            i += 1;
            let mut w = String::new();
            let mut closed = false;
            while i < cs.len() {
                if cs[i] == '\\' && i + 1 < cs.len() && (cs[i + 1] == q || cs[i + 1] == '\\') {
                    w.push(cs[i + 1]);
                    i += 2;
                } else if cs[i] == q {
                    closed = true;
                    i += 1;
                    break;
                } else {
                    w.push(cs[i]);
                    i += 1;
                }
            }
            if !closed {
                return Err("unterminated quote".into());
            }
            out.push(Tok::Word(w));
        } else {
            let st = i;
            while i < cs.len() && !cs[i].is_whitespace() && cs[i] != '(' && cs[i] != ')' {
                i += 1;
            }
            let w: String = cs[st..i].iter().collect();
            if let Some(f) = w.strip_prefix('~') {
                out.push(Tok::Flag(f.to_string()));
            } else {
                out.push(Tok::Word(w));
            }
        }
    }
    Ok(out)
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

fn rx(p: &str) -> Result<Regex, String> {
    RegexBuilder::new(p)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("bad regex '{p}': {e}"))
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.peek() == Some(&Tok::Or) {
            self.i += 1;
            let r = self.and()?;
            l = Expr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while self.peek() == Some(&Tok::And) {
            self.i += 1;
            let r = self.not()?;
            l = Expr::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn not(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Tok::Not) {
            self.i += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.atom()
    }
    fn arg(&mut self, flag: &str) -> Result<String, String> {
        match self.t.get(self.i).cloned() {
            Some(Tok::Word(w)) => {
                self.i += 1;
                Ok(w)
            }
            _ => Err(format!("~{flag} needs an argument")),
        }
    }
    fn atom(&mut self) -> Result<Expr, String> {
        let t = self.t.get(self.i).cloned().ok_or_else(|| "unexpected end of filter".to_string())?;
        self.i += 1;
        match t {
            Tok::LP => {
                let e = self.or()?;
                if self.peek() != Some(&Tok::RP) {
                    return Err("missing ')'".into());
                }
                self.i += 1;
                Ok(e)
            }
            Tok::Flag(f) => match f.as_str() {
                "all" | "a" => Ok(Expr::All),
                "q" => Ok(Expr::Req),
                "s" => Ok(Expr::Resp),
                "u" => Ok(Expr::Url(rx(&self.arg(&f)?)?)),
                "d" => Ok(Expr::Domain(rx(&self.arg(&f)?)?)),
                "m" => Ok(Expr::Method(self.arg(&f)?)),
                "h" => Ok(Expr::Header(rx(&self.arg(&f)?)?)),
                "t" => Ok(Expr::ContentType(rx(&self.arg(&f)?)?)),
                "b" => Ok(Expr::Body(rx(&self.arg(&f)?)?)),
                "c" => {
                    let a = self.arg(&f)?;
                    let ok = a.split(',').all(|p| {
                        let p = p.trim();
                        !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    });
                    if !ok {
                        return Err(format!("bad status spec '{a}'"));
                    }
                    Ok(Expr::Status(a))
                }
                other => Err(format!("unknown filter ~{other}")),
            },
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

/// Parse a filter. Empty/blank text means `~all`.
pub fn parse(s: &str) -> Result<Expr, String> {
    if s.trim().is_empty() {
        return Ok(Expr::All);
    }
    let mut p = P { t: lex(s)?, i: 0 };
    let e = p.or()?;
    if p.i != p.t.len() {
        return Err(format!("unexpected token {:?}", p.t[p.i]));
    }
    Ok(e)
}

type Cache = Mutex<HashMap<String, Option<Arc<Expr>>>>;
static CACHE: OnceLock<Cache> = OnceLock::new();

/// Parse with a small cache (the proxy evaluates the same rule text on every exchange).
/// Unparsable text caches as `None` and never matches.
pub fn parse_cached(s: &str) -> Option<Arc<Expr>> {
    let m = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().ok()?;
    if let Some(v) = g.get(s) {
        return v.clone();
    }
    if g.len() > 512 {
        g.clear();
    }
    let v = parse(s).ok().map(Arc::new);
    g.insert(s.to_string(), v.clone());
    v
}

/// Evaluate rule text against a context; parse errors never match.
pub fn eval_str(s: &str, c: &Ctx) -> bool {
    parse_cached(s).map(|e| e.eval(c)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(m: &'a str, u: &'a str) -> Ctx<'a> {
        Ctx { method: m, url: u, status: 0, response: false, headers: &[], body: "" }
    }
    fn resp<'a>(m: &'a str, u: &'a str, s: u16) -> Ctx<'a> {
        Ctx { method: m, url: u, status: s, response: true, headers: &[], body: "" }
    }
    fn ev(f: &str, c: &Ctx) -> bool {
        parse(f).unwrap().eval(c)
    }

    #[test]
    fn flow_eval() {
        // A completed flow: both ~q and ~s hold; headers/body are the merged request+response.
        let h = [("content-type".to_string(), "application/json".to_string())];
        let c = Ctx { method: "POST", url: "http://h/api", status: 500, response: true, headers: &h, body: "oops" };
        assert!(parse("~q & ~s").unwrap().eval_flow(&c)); // both sides present
        assert!(parse("~c 5xx").unwrap().eval_flow(&c));
        assert!(parse("~m POST & ~t json").unwrap().eval_flow(&c));
        assert!(parse("~b oops | ~m GET").unwrap().eval_flow(&c));
        assert!(!parse("~c 2xx").unwrap().eval_flow(&c));
        // No response yet (status 0): ~s is false, ~q still true.
        let c0 = Ctx { status: 0, response: false, ..c };
        assert!(!parse("~s").unwrap().eval_flow(&c0));
        assert!(parse("~q").unwrap().eval_flow(&c0));
        assert!(!parse("~c 500").unwrap().eval_flow(&c0));
    }

    #[test]
    fn body_matching() {
        let c = Ctx { method: "POST", url: "http://h/", status: 200, response: true, headers: &[], body: "{\"error\":\"boom\"}" };
        assert!(ev("~b boom", &c));
        assert!(ev("~b \"error.:.boom\"", &c)); // regex
        assert!(!ev("~b missing", &c));
        assert!(parse("~s & ~b boom").unwrap().uses_body());
        assert!(!parse("~s & ~c 500").unwrap().uses_body());
        assert!(parse("!~b x").unwrap().uses_body()); // negation still inspects the body
    }

    #[test]
    fn header_matching() {
        let h = [("Content-Type".to_string(), "application/json".to_string()), ("Authorization".to_string(), "Bearer x".to_string())];
        let c = Ctx { method: "GET", url: "http://h/", status: 0, response: false, headers: &h, body: "" };
        assert!(ev("~h authorization", &c)); // case-insensitive, matches the name
        assert!(ev("~h \"application/json\"", &c)); // matches the value
        assert!(ev("~h \"content-type: application/json\"", &c)); // matches "name: value"
        assert!(!ev("~h x-missing", &c));
        assert!(ev("~h bearer & ~m GET", &c));
        assert!(ev("~t json", &c)); // Content-Type shortcut
        assert!(ev("~t \"application/json\"", &c));
        assert!(!ev("~t xml", &c));
        // No headers in context -> ~h never matches, but other terms still work.
        assert!(!ev("~h authorization", &req("GET", "http://h/")));
    }

    #[test]
    fn basics() {
        let c = req("POST", "https://api.example.com/v1/login?x=1");
        assert!(ev("", &c));
        assert!(ev("~all", &c));
        assert!(ev("~u /v1/", &c));
        assert!(ev("~m post", &c));
        assert!(!ev("~m GET", &c));
        assert!(ev("~d ^api\\.example\\.com$", &c));
        assert!(ev("~q", &c));
        assert!(!ev("~s", &c));
        assert!(!ev("~c 200", &c));
    }

    #[test]
    fn status_and_stage() {
        let c = resp("GET", "http://a.test/x", 503);
        assert!(ev("~s & ~c 5xx", &c));
        assert!(ev("~c 400-599", &c));
        assert!(ev("~c 401,503", &c));
        assert!(!ev("~c 404", &c));
        assert!(!ev("~q", &c));
    }

    #[test]
    fn boolean_precedence() {
        let c = req("GET", "http://a.test/x");
        assert!(ev("~m POST | ~m GET & ~u /x", &c));
        assert!(!ev("(~m POST | ~m GET) & ~u /y", &c));
        assert!(ev("!~m POST", &c));
        assert!(ev("!!~m GET", &c));
        assert!(ev("~u \"a|zzz\"", &c));
        assert!(!ev("~u \"q|zzz\"", &c));
    }

    #[test]
    fn errors() {
        for bad in ["~u", "~z foo", "(~q", "~q )", "~q &", "~u (", "~u \"abc", "~c !", "~u \"[\""] {
            assert!(parse(bad).is_err(), "should fail: {bad}");
        }
        assert!(!eval_str("~u \"[\"", &req("GET", "http://x/")));
    }

    #[test]
    fn host_extraction() {
        assert_eq!(host_of("https://u:p@h.io:8443/p?q=1"), "h.io");
        assert_eq!(host_of("http://[::1]:80/"), "::1");
    }
}

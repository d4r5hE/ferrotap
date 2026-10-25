//! G3 inspectors: decoded bodies, cookies/params/timings/sizes views, body search/wrap/save, decode tool.
//! Owned by one builder; the app calls `inspect_menu` from the toolbar and `inspect_windows` every frame.
//!
//! The first half of this file is pure logic (parsers, pretty-printers, decoders) covered by
//! `tests/inspect_*.rs`; the second half holds the egui helpers used by the request/response inspectors.
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use crate::model::SessionRecord;
use crate::ui::GridApp;

// ---------------------------------------------------------------------------------------------
// URL / query / form helpers
// ---------------------------------------------------------------------------------------------

/// Percent-decode `s`. When `plus_is_space` is set (query strings, urlencoded forms) `+` becomes a space.
/// Invalid escapes are kept literally; the decoded bytes are interpreted as UTF-8 (lossy).
pub fn url_decode(s: &str, plus_is_space: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = (b[i + 1] as char).to_digit(16);
                let l = (b[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' if plus_is_space => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode everything except RFC 3986 unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// Parse `a=1&b=x%20y` into decoded (name, value) pairs, keeping order and duplicates.
pub fn parse_urlencoded(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (url_decode(k, true), url_decode(v, true)),
            None => (url_decode(p, true), String::new()),
        })
        .collect()
}

/// Query-string parameters of a URL (fragment ignored).
pub fn parse_query(url: &str) -> Vec<(String, String)> {
    let no_frag = url.split('#').next().unwrap_or("");
    match no_frag.split_once('?') {
        Some((_, q)) => parse_urlencoded(q),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// Cookies
// ---------------------------------------------------------------------------------------------

/// Pairs from a request `Cookie:` header (`a=1; b=2`).
pub fn parse_cookie_header(v: &str) -> Vec<(String, String)> {
    v.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect()
}

/// One parsed `Set-Cookie` response header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetCookie {
    pub name: String,
    pub value: String,
    pub domain: Option<String>,
    pub path: Option<String>,
    pub expires: Option<String>,
    pub max_age: Option<String>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<String>,
}

/// Parse a single `Set-Cookie` value (`name=value; Path=/; HttpOnly; ...`). Attribute names are case-insensitive.
pub fn parse_set_cookie(line: &str) -> Option<SetCookie> {
    let mut parts = line.split(';');
    let first = parts.next()?.trim();
    if first.is_empty() {
        return None;
    }
    let (name, value) = match first.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (String::new(), first.to_string()),
    };
    let mut c = SetCookie { name, value, ..Default::default() };
    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (k, v) = match attr.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim().to_string())),
            None => (attr, None),
        };
        match k.to_ascii_lowercase().as_str() {
            "domain" => c.domain = v,
            "path" => c.path = v,
            "expires" => c.expires = v,
            "max-age" => c.max_age = v,
            "samesite" => c.same_site = v,
            "httponly" => c.http_only = true,
            "secure" => c.secure = true,
            _ => {}
        }
    }
    Some(c)
}

/// Split a stored `Set-Cookie` header that may hold several cookies (joined by newlines or commas)
/// into individual cookie strings. A comma only separates cookies when it is followed by `token=`,
/// so `Expires=Wed, 21 Oct 2015 07:28:00 GMT` stays intact.
pub fn split_set_cookie_header(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in v.lines() {
        let mut start = 0;
        let bytes = line.as_bytes();
        for (i, &ch) in bytes.iter().enumerate() {
            if ch != b',' {
                continue;
            }
            let rest = &line[i + 1..];
            let rest_t = rest.trim_start();
            let tok_len = rest_t
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(b))
                .count();
            if tok_len > 0 && rest_t.as_bytes().get(tok_len) == Some(&b'=') {
                let piece = line[start..i].trim();
                if !piece.is_empty() {
                    out.push(piece.to_string());
                }
                start = i + 1;
            }
        }
        let piece = line[start..].trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
    }
    out
}

/// All `Set-Cookie` cookies of a response header map.
pub fn response_set_cookies(h: &BTreeMap<String, String>) -> Vec<SetCookie> {
    h.iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .flat_map(|(_, v)| split_set_cookie_header(v))
        .filter_map(|c| parse_set_cookie(&c))
        .collect()
}

fn header<'a>(h: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------------------------------------
// Web forms
// ---------------------------------------------------------------------------------------------

/// One part of a `multipart/form-data` body (or one urlencoded field).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormPart {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size: usize,
    /// Text of the part (for display; empty for file parts larger than 4 KiB).
    pub value: String,
}

/// Value of a `key=value` parameter inside a header such as Content-Type or Content-Disposition.
pub fn header_param(v: &str, key: &str) -> Option<String> {
    for p in v.split(';').skip(1) {
        if let Some((k, val)) = p.split_once('=') {
            if k.trim().eq_ignore_ascii_case(key) {
                let val = val.trim();
                let val = val.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(val);
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Parse a multipart body given its Content-Type (which carries the boundary).
pub fn parse_multipart(body: &str, content_type: &str) -> Vec<FormPart> {
    let Some(boundary) = header_param(content_type, "boundary") else { return Vec::new() };
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    for chunk in body.split(delim.as_str()).skip(1) {
        if chunk.starts_with("--") {
            break; // closing delimiter
        }
        let chunk = chunk.strip_prefix("\r\n").or_else(|| chunk.strip_prefix('\n')).unwrap_or(chunk);
        let (head, data) = match chunk.find("\r\n\r\n") {
            Some(i) => (&chunk[..i], &chunk[i + 4..]),
            None => match chunk.find("\n\n") {
                Some(i) => (&chunk[..i], &chunk[i + 2..]),
                None => (chunk, ""),
            },
        };
        let data = data.strip_suffix("\r\n").or_else(|| data.strip_suffix('\n')).unwrap_or(data);
        let mut part = FormPart { size: data.len(), ..Default::default() };
        for line in head.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let k = k.trim();
            if k.eq_ignore_ascii_case("content-disposition") {
                part.name = header_param(v, "name").unwrap_or_default();
                part.filename = header_param(v, "filename");
            } else if k.eq_ignore_ascii_case("content-type") {
                part.content_type = Some(v.trim().to_string());
            }
        }
        part.value = if part.filename.is_some() && data.len() > 4096 { String::new() } else { data.to_string() };
        parts.push(part);
    }
    parts
}

/// Form view of a request: urlencoded fields or multipart parts, or `None` when the body is not a form.
pub fn parse_form(s: &SessionRecord) -> Option<Vec<FormPart>> {
    let ct = header(&s.request_headers, "content-type").unwrap_or("");
    let lower = ct.to_ascii_lowercase();
    if lower.starts_with("application/x-www-form-urlencoded") {
        Some(
            parse_urlencoded(s.request_body.trim())
                .into_iter()
                .map(|(name, value)| FormPart { name, size: value.len(), value, ..Default::default() })
                .collect(),
        )
    } else if lower.starts_with("multipart/") {
        Some(parse_multipart(&s.request_body, ct))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Pretty printers: XML / HTML / JavaScript
// ---------------------------------------------------------------------------------------------

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
];
const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style", "pre", "textarea"];

#[derive(Debug)]
enum Tok<'a> {
    Open(&'a str, String),
    Close(&'a str, String),
    Single(&'a str),
    Text(&'a str),
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches(['<', '/'])
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// End (exclusive) of the tag starting at `i` (`s[i] == '<'`), honouring quoted attribute values.
fn tag_end(s: &str, i: usize) -> usize {
    let rest = &s[i..];
    for (prefix, end) in [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")] {
        if rest.starts_with(prefix) {
            return rest.find(end).map(|e| i + e + end.len()).unwrap_or(s.len());
        }
    }
    let mut quote: Option<char> = None;
    for (j, c) in rest.char_indices().skip(1) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '>' => return i + j + 1,
            None => {}
        }
    }
    s.len()
}

fn tokenize_markup(s: &str, html: bool) -> Vec<Tok<'_>> {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let is_tag = s[i..].starts_with('<')
            && s[i + 1..].chars().next().map(|c| c.is_alphabetic() || "/!?".contains(c)).unwrap_or(false);
        if is_tag {
            let e = tag_end(s, i);
            let t = &s[i..e];
            if t.starts_with("</") {
                toks.push(Tok::Close(t, tag_name(t)));
            } else if t.starts_with("<!") || t.starts_with("<?") || t.ends_with("/>") {
                toks.push(Tok::Single(t));
            } else {
                let name = tag_name(t);
                if html && VOID_ELEMENTS.contains(&name.as_str()) {
                    toks.push(Tok::Single(t));
                } else if html && RAW_TEXT_ELEMENTS.contains(&name.as_str()) {
                    toks.push(Tok::Open(t, name.clone()));
                    let close = format!("</{name}");
                    let lower_rest = s[e..].to_ascii_lowercase();
                    let ce = lower_rest.find(&close).map(|x| e + x).unwrap_or(s.len());
                    if ce > e {
                        toks.push(Tok::Text(&s[e..ce]));
                    }
                    i = ce;
                    continue;
                } else {
                    toks.push(Tok::Open(t, name));
                }
            }
            i = e;
        } else {
            // step over the whole first character (text can start with a multi-byte one)
            let step = s[i..].chars().next().map_or(1, char::len_utf8);
            let next = s[i + step..].find('<').map(|x| i + step + x).unwrap_or(s.len());
            toks.push(Tok::Text(&s[i..next]));
            i = next;
        }
    }
    toks
}

/// Deepest indentation the pretty-printers use: past it, deeper levels line up (deeply nested
/// input would otherwise grow the output quadratically).
pub const MAX_INDENT: usize = 64;

/// Re-indent XML (`html == false`) or HTML (`html == true`, knows void and raw-text elements) with two spaces
/// per level. `<a>short text</a>` stays on one line. Never fails: malformed input is indented best-effort.
pub fn pretty_markup(s: &str, html: bool) -> String {
    let toks = tokenize_markup(s, html);
    let mut out = String::new();
    let mut depth: usize = 0;
    let push = |out: &mut String, depth: usize, text: &str| {
        out.push_str(&"  ".repeat(depth.min(MAX_INDENT)));
        out.push_str(text);
        out.push('\n');
    };
    let mut k = 0;
    while k < toks.len() {
        match &toks[k] {
            Tok::Open(t, name) => {
                // Compact form: <x>text</x>
                if let (Some(Tok::Text(txt)), Some(Tok::Close(ct, cn))) = (toks.get(k + 1), toks.get(k + 2)) {
                    if cn == name && !txt.trim().contains('\n') && txt.trim().len() <= 120 {
                        push(&mut out, depth, &format!("{t}{}{ct}", txt.trim()));
                        k += 3;
                        continue;
                    }
                }
                if let Some(Tok::Close(ct, cn)) = toks.get(k + 1) {
                    if cn == name {
                        push(&mut out, depth, &format!("{t}{ct}"));
                        k += 2;
                        continue;
                    }
                }
                push(&mut out, depth, t);
                if html && RAW_TEXT_ELEMENTS.contains(&name.as_str()) {
                    if let Some(Tok::Text(txt)) = toks.get(k + 1) {
                        for line in txt.lines().map(str::trim).filter(|l| !l.is_empty()) {
                            push(&mut out, depth + 1, line);
                        }
                        k += 1;
                    }
                    if let Some(Tok::Close(ct, _)) = toks.get(k + 1) {
                        push(&mut out, depth, ct);
                        k += 1;
                    }
                } else {
                    depth += 1;
                }
            }
            Tok::Close(t, _) => {
                depth = depth.saturating_sub(1);
                push(&mut out, depth, t);
            }
            Tok::Single(t) => push(&mut out, depth, t),
            Tok::Text(t) => {
                for line in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    push(&mut out, depth, line);
                }
            }
        }
        k += 1;
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Re-indent JavaScript (or CSS): breaks after `{`, `}` and `;` (outside parentheses,
/// strings, template literals and comments) and indents two spaces per brace level.
pub fn pretty_js(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth: usize = 0;
    let mut paren: usize = 0;
    let chars: Vec<char> = s.chars().collect();
    let flush = |lines: &mut Vec<String>, cur: &mut String, depth: usize| {
        let t = cur.trim();
        if !t.is_empty() {
            lines.push(format!("{}{}", "  ".repeat(depth.min(MAX_INDENT)), t));
        }
        cur.clear();
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' | '`' => {
                cur.push(c);
                i += 1;
                while i < chars.len() {
                    let d = chars[i];
                    cur.push(d);
                    i += 1;
                    if d == '\\' && i < chars.len() {
                        cur.push(chars[i]);
                        i += 1;
                    } else if d == c {
                        break;
                    }
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    cur.push(chars[i]);
                    i += 1;
                }
                flush(&mut lines, &mut cur, depth);
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                cur.push_str("/*");
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    cur.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    cur.push_str("*/");
                    i += 2;
                }
                continue;
            }
            '(' => {
                paren += 1;
                cur.push(c);
            }
            ')' => {
                paren = paren.saturating_sub(1);
                cur.push(c);
            }
            '{' => {
                cur.push(c);
                flush(&mut lines, &mut cur, depth);
                depth += 1;
            }
            '}' => {
                flush(&mut lines, &mut cur, depth);
                depth = depth.saturating_sub(1);
                cur.push(c);
                // keep `};`, `},` and `})` on the closing line
                while let Some(&n) = chars.get(i + 1) {
                    if n == ';' || n == ',' || n == ')' {
                        if n == ')' {
                            paren = paren.saturating_sub(1);
                        }
                        cur.push(n);
                        i += 1;
                    } else {
                        break;
                    }
                }
                flush(&mut lines, &mut cur, depth);
            }
            ';' if paren == 0 => {
                cur.push(c);
                flush(&mut lines, &mut cur, depth);
            }
            '\n' | '\r' => flush(&mut lines, &mut cur, depth),
            c if c.is_whitespace() => {
                if !cur.is_empty() && !cur.ends_with(' ') {
                    cur.push(' ');
                }
            }
            c => cur.push(c),
        }
        i += 1;
    }
    flush(&mut lines, &mut cur, depth);
    lines.join("\n")
}

// ---------------------------------------------------------------------------------------------
// Content-Encoding (gzip / deflate / br / zstd)
// ---------------------------------------------------------------------------------------------

/// Result of trying to auto-decode a stored body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyDecode {
    /// No Content-Encoding (or only `identity`): the stored body is shown as is.
    NotEncoded,
    /// The stored bytes were decompressed; `encodings` lists the codecs undone (in decode order).
    Decoded { encodings: Vec<String>, text: String },
    /// Decoding failed. `already_decoded` is true when the stored text looks like readable text
    /// (e.g. an import that saved the decoded body but kept the header).
    Failed { encodings: Vec<String>, reason: String, already_decoded: bool },
}

/// Windows-1252 code point for a byte (Latin-1 outside the 0x80-0x9F block, which is remapped).
fn win1252(b: u8) -> char {
    const HI: [u16; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D,
        0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
        0x0153, 0x009D, 0x017E, 0x0178,
    ];
    if (0x80..=0x9F).contains(&b) {
        char::from_u32(HI[(b - 0x80) as usize] as u32).unwrap_or(b as char)
    } else {
        b as char
    }
}

fn decode_utf16(raw: &[u8], cs: &str) -> String {
    let be = cs == "utf-16be" || raw.starts_with(&[0xFE, 0xFF]);
    let body = if raw.starts_with(&[0xFF, 0xFE]) || raw.starts_with(&[0xFE, 0xFF]) { &raw[2..] } else { raw };
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Strip a leading XSSI protection prefix some APIs prepend to JSON.
fn strip_xssi(s: &str) -> &str {
    let s = s.trim_start();
    for guard in [")]}'", "while(1);", "for(;;);", "])}while(1);</x>"] {
        if let Some(rest) = s.strip_prefix(guard) {
            return rest.trim_start();
        }
    }
    s
}

/// If `s` is a JSONP call `ident(...)` (with an optional trailing `;`), return (callback, inner).
fn jsonp_split(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let s = s.strip_suffix(';').unwrap_or(s).trim_end();
    if !s.ends_with(')') {
        return None;
    }
    let open = s.find('(')?;
    let cb = s[..open].trim();
    let mut chars = cb.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$');
    if !first_ok || !cb.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.') {
        return None;
    }
    Some((cb, s[open + 1..s.len() - 1].trim()))
}

/// Pretty-print a JSON body, also handling JSONP wrappers, XSSI guard prefixes and NDJSON (one
/// JSON value per line). Returns a readable error note (followed by the raw text) if it is not JSON.
pub fn pretty_json(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "(Empty body)".to_string();
    }
    let stripped = strip_xssi(raw.trim());
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stripped) {
        return serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string());
    }
    if let Some((cb, inner)) = jsonp_split(stripped) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(inner) {
            return format!("{cb}(\n{}\n);", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
    }
    let lines: Vec<&str> = stripped.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() > 1 {
        let parsed: Option<Vec<String>> = lines
            .iter()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).ok().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()))
            .collect();
        if let Some(p) = parsed {
            return p.join("\n");
        }
    }
    format!("// Not valid JSON\n\n{raw}")
}

/// The charset token of a Content-Type value, lowercased (e.g. "iso-8859-1").
pub fn charset_label(content_type: Option<&str>) -> Option<String> {
    header_param(content_type?, "charset").map(|c| c.trim().trim_matches('"').to_ascii_lowercase())
}

/// Decode bytes to display text using a non-UTF-8 charset named in the Content-Type. Returns `None`
/// for UTF-8 / US-ASCII, an absent or unknown charset, and multi-byte charsets we do not handle (the
/// caller then falls back to lossy UTF-8). Handles ISO-8859-1 / Latin-1, Windows-1252 and UTF-16.
pub fn decode_charset(raw: &[u8], content_type: Option<&str>) -> Option<String> {
    let cs = charset_label(content_type)?;
    match cs.as_str() {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => None,
        "iso-8859-1" | "iso8859-1" | "iso_8859-1" | "latin1" | "latin-1" | "l1" | "cp819" => Some(raw.iter().map(|&b| b as char).collect()),
        "windows-1252" | "cp1252" | "x-cp1252" => Some(raw.iter().map(|&b| win1252(b)).collect()),
        "utf-16" | "utf-16le" | "utf-16be" | "ucs-2" | "unicode" => Some(decode_utf16(raw, &cs)),
        _ => None,
    }
}

/// Turn decoded bytes into display text, honouring a non-UTF-8 charset when we recognise one.
fn bytes_to_text(raw: &[u8], content_type: Option<&str>) -> String {
    decode_charset(raw, content_type).unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned())
}

/// Try to undo `content_encoding` on the stored body. Ferrotap stores bodies as UTF-8 text, so a
/// compressed body only survives when its bytes were not altered by lossy conversion; otherwise this
/// reports honestly that the original bytes are not available. `content_type` selects the charset for
/// turning the decoded bytes into text.
pub fn decode_body(stored: &str, content_encoding: Option<&str>, content_type: Option<&str>) -> BodyDecode {
    decode_body_bytes(stored.as_bytes(), content_encoding, content_type)
}

/// Like `decode_body`, on the exact captured bytes (`SessionRecord::response_bytes`).
pub fn decode_body_bytes(raw: &[u8], content_encoding: Option<&str>, content_type: Option<&str>) -> BodyDecode {
    let stored = String::from_utf8_lossy(raw);
    let stored_was_text = std::str::from_utf8(raw).is_ok();
    let Some(ce) = content_encoding.map(str::trim).filter(|c| !c.is_empty()) else { return BodyDecode::NotEncoded };
    let names: Vec<String> = ce
        .split(',')
        .map(|x| x.trim().to_ascii_lowercase())
        .filter(|x| !x.is_empty() && x != "identity")
        .collect();
    if names.is_empty() {
        return BodyDecode::NotEncoded;
    }
    let lossy = stored_was_text && stored.contains('\u{FFFD}');
    let looks_text = stored_was_text && !lossy && stored.chars().take(4096).all(|c| !c.is_control() || c.is_whitespace());
    let codings = match crate::decode::parse_content_encodings(ce) {
        Ok(c) => c,
        Err(e) => return BodyDecode::Failed { encodings: names, reason: e.to_string(), already_decoded: looks_text },
    };
    match decode_codings(raw, &codings) {
        Ok((applied, bytes)) => BodyDecode::Decoded { encodings: applied, text: bytes_to_text(&bytes, content_type) },
        Err(e) => {
            let reason = if lossy {
                format!("{e} (the original compressed bytes were not kept: the body was captured as lossy UTF-8 text)")
            } else {
                e
            };
            BodyDecode::Failed { encodings: names, reason, already_decoded: looks_text }
        }
    }
}

fn decode_codings(data: &[u8], codings: &[crate::decode::ContentCoding]) -> Result<(Vec<String>, Vec<u8>), String> {
    let limits = crate::decode::DecoderLimits::default();
    let mut bytes = data.to_vec();
    let mut applied = Vec::new();
    for c in codings.iter().rev() {
        if *c == crate::decode::ContentCoding::Identity {
            continue;
        }
        bytes = crate::decode::decompress_single(&bytes, *c, &limits).map_err(|e| e.to_string())?;
        applied.push(c.as_str().to_string());
    }
    Ok((applied, bytes))
}

/// Undo a Content-Encoding header value (e.g. `gzip`, `br`, `deflate, zstd`) on raw bytes.
/// Returns the codecs undone (in decode order) and the decoded bytes.
pub fn decode_bytes(data: &[u8], content_encoding: &str) -> Result<(Vec<String>, Vec<u8>), String> {
    let codings = crate::decode::parse_content_encodings(content_encoding).map_err(|e| e.to_string())?;
    decode_codings(data, &codings)
}

// ---------------------------------------------------------------------------------------------
// Decode tool: base64 / URL / HTML entities / hex / JWT
// ---------------------------------------------------------------------------------------------

/// Show bytes as UTF-8 text when valid, otherwise as a hex dump.
pub fn bytes_to_display(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => format!("(binary, {} bytes)\n{}", b.len(), hex_encode(b)),
    }
}

/// Decode standard or URL-safe base64, with or without padding; whitespace is ignored.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    use base64::Engine;
    let norm: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    let eng = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    eng.decode(norm.as_bytes()).map_err(|e| format!("Invalid base64: {e}"))
}

pub fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// Lower-case hex bytes separated by spaces.
pub fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// Decode hex text; accepts spaces, `:`/`-`/`,` separators and `0x` prefixes.
pub fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = s
        .split(|c: char| c.is_whitespace() || c == ',' || c == ':' || c == '-')
        .map(|w| w.trim_start_matches("0x").trim_start_matches("0X"))
        .collect();
    if !cleaned.is_ascii() {
        return Err("Invalid hex: non-hex characters".into());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err("Invalid hex: odd number of digits".into());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|_| format!("Invalid hex digits '{}'", &cleaned[i..i + 2])))
        .collect()
}

/// Decode HTML character references: common named entities, `&#NN;` and `&#xHH;`. Unknown ones are kept.
pub fn html_decode(s: &str) -> String {
    const NAMED: &[(&str, &str)] = &[
        ("amp", "&"), ("lt", "<"), ("gt", ">"), ("quot", "\""), ("apos", "'"), ("nbsp", "\u{a0}"),
        ("copy", "\u{a9}"), ("reg", "\u{ae}"), ("trade", "\u{2122}"), ("hellip", "\u{2026}"),
        ("mdash", "\u{2014}"), ("ndash", "\u{2013}"), ("laquo", "\u{ab}"), ("raquo", "\u{bb}"),
        ("euro", "\u{20ac}"), ("pound", "\u{a3}"), ("yen", "\u{a5}"), ("cent", "\u{a2}"), ("sect", "\u{a7}"),
        ("deg", "\u{b0}"), ("times", "\u{d7}"), ("divide", "\u{f7}"), ("middot", "\u{b7}"), ("bull", "\u{2022}"),
    ];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let semi = after.find(';').filter(|&i| i <= 32);
        let mut done = false;
        if let Some(i) = semi {
            let ent = &after[..i];
            let ch = if let Some(num) = ent.strip_prefix('#') {
                let v = if let Some(h) = num.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok() } else { num.parse::<u32>().ok() };
                v.and_then(char::from_u32).map(|c| c.to_string())
            } else {
                NAMED.iter().find(|(n, _)| *n == ent).map(|(_, v)| v.to_string())
            };
            if let Some(ch) = ch {
                out.push_str(&ch);
                rest = &after[i + 1..];
                done = true;
            }
        }
        if !done {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn html_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A JWT split into its pretty-printed parts. The signature is NOT verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwt {
    pub header: String,
    pub payload: String,
    pub signature_bytes: usize,
    /// Human-readable `iat` / `nbf` / `exp` claims (UTC), if present.
    pub times: Vec<(String, String)>,
}

fn pretty_json_bytes(b: &[u8]) -> String {
    match serde_json::from_slice::<serde_json::Value>(b) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
        Err(_) => bytes_to_display(b),
    }
}

/// Decode a JWT (`header.payload.signature`, optionally prefixed by `Bearer `).
pub fn jwt_decode(token: &str) -> Result<Jwt, String> {
    let t = token.trim();
    let t = t.strip_prefix("Bearer ").or_else(|| t.strip_prefix("bearer ")).unwrap_or(t).trim();
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("A JWT has 3 dot-separated parts; found {}", parts.len()));
    }
    let h = base64_decode(parts[0]).map_err(|e| format!("Header: {e}"))?;
    let p = base64_decode(parts[1]).map_err(|e| format!("Payload: {e}"))?;
    let sig = base64_decode(parts[2]).map(|s| s.len()).unwrap_or(0);
    let mut times = Vec::new();
    if let Ok(serde_json::Value::Object(m)) = serde_json::from_slice::<serde_json::Value>(&p) {
        for k in ["iat", "nbf", "exp"] {
            if let Some(n) = m.get(k).and_then(|v| v.as_i64()) {
                if let Some(dt) = chrono::DateTime::from_timestamp(n, 0) {
                    times.push((k.to_string(), dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()));
                }
            }
        }
    }
    Ok(Jwt { header: pretty_json_bytes(&h), payload: pretty_json_bytes(&p), signature_bytes: sig, times })
}

/// Operations offered by the Decode tool window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeOp {
    Base64Decode,
    Base64Encode,
    UrlDecode,
    UrlEncode,
    HtmlDecode,
    HtmlEncode,
    HexDecode,
    HexEncode,
    Jwt,
    Timestamp,
    JsonPretty,
    JsonMinify,
}

impl DecodeOp {
    pub const ALL: [DecodeOp; 12] = [
        DecodeOp::Base64Decode, DecodeOp::Base64Encode, DecodeOp::UrlDecode, DecodeOp::UrlEncode,
        DecodeOp::HtmlDecode, DecodeOp::HtmlEncode, DecodeOp::HexDecode, DecodeOp::HexEncode, DecodeOp::Jwt,
        DecodeOp::Timestamp, DecodeOp::JsonPretty, DecodeOp::JsonMinify,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DecodeOp::Base64Decode => "Base64 decode",
            DecodeOp::Base64Encode => "Base64 encode",
            DecodeOp::UrlDecode => "URL decode",
            DecodeOp::UrlEncode => "URL encode",
            DecodeOp::HtmlDecode => "HTML entities decode",
            DecodeOp::HtmlEncode => "HTML entities encode",
            DecodeOp::HexDecode => "Hex decode",
            DecodeOp::HexEncode => "Hex encode",
            DecodeOp::Jwt => "JWT decode",
            DecodeOp::Timestamp => "Unix timestamp -> date",
            DecodeOp::JsonPretty => "JSON prettify",
            DecodeOp::JsonMinify => "JSON minify",
        }
    }
}

/// Run one Decode tool operation; errors are returned as readable text.
pub fn run_decode(op: DecodeOp, input: &str) -> Result<String, String> {
    Ok(match op {
        DecodeOp::Base64Decode => bytes_to_display(&base64_decode(input)?),
        DecodeOp::Base64Encode => base64_encode(input.as_bytes()),
        DecodeOp::UrlDecode => url_decode(input.trim(), true),
        DecodeOp::UrlEncode => url_encode(input),
        DecodeOp::HtmlDecode => html_decode(input),
        DecodeOp::HtmlEncode => html_encode(input),
        DecodeOp::HexDecode => bytes_to_display(&hex_decode(input)?),
        DecodeOp::HexEncode => hex_encode(input.as_bytes()),
        DecodeOp::Jwt => {
            let j = jwt_decode(input)?;
            let mut s = format!("// Header\n{}\n\n// Payload\n{}\n", j.header, j.payload);
            for (k, v) in &j.times {
                s.push_str(&format!("// {k} = {v}\n"));
            }
            s.push_str(&format!("\n// Signature: {} bytes (not verified)", j.signature_bytes));
            s
        }
        DecodeOp::Timestamp => {
            let n: i64 = input.trim().parse().map_err(|_| "not an integer Unix timestamp".to_string())?;
            // Guess the unit from the magnitude: >= 1e15 microseconds, >= 1e12 milliseconds, else seconds.
            let dt = if n.abs() >= 1_000_000_000_000_000 {
                chrono::DateTime::from_timestamp_micros(n)
            } else if n.abs() >= 1_000_000_000_000 {
                chrono::DateTime::from_timestamp_millis(n)
            } else {
                chrono::DateTime::from_timestamp(n, 0)
            };
            let dt = dt.ok_or_else(|| "timestamp out of range".to_string())?;
            format!("{}\n{} UTC", dt.to_rfc3339(), dt.format("%Y-%m-%d %H:%M:%S"))
        }
        DecodeOp::JsonPretty => pretty_json(input),
        DecodeOp::JsonMinify => {
            let v: serde_json::Value = serde_json::from_str(input.trim()).map_err(|e| format!("not valid JSON: {e}"))?;
            serde_json::to_string(&v).map_err(|e| e.to_string())?
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Search, sizes, timings, saving
// ---------------------------------------------------------------------------------------------

/// Byte ranges of every non-overlapping match of `needle` in `hay` (ASCII case-insensitive unless `case_sensitive`).
pub fn find_matches(hay: &str, needle: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let (h, n): (Cow<str>, Cow<str>) = if case_sensitive {
        (Cow::Borrowed(hay), Cow::Borrowed(needle))
    } else {
        (Cow::Owned(hay.to_ascii_lowercase()), Cow::Owned(needle.to_ascii_lowercase()))
    };
    h.match_indices(n.as_ref()).map(|(i, m)| i..i + m.len()).collect()
}

/// Bytes of a header block as sent on the wire (`Name: value\r\n` per header plus the final CRLF),
/// plus the first line (and its CRLF) when given.
pub fn header_bytes(first_line: &str, h: &BTreeMap<String, String>) -> usize {
    let mut n = if first_line.is_empty() { 0 } else { first_line.len() + 2 };
    for (k, v) in h {
        n += k.len() + 2 + v.len() + 2;
    }
    n + 2
}

/// Size figures for the Sizes tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sizes {
    pub request_header_bytes: usize,
    pub request_body_bytes: usize,
    pub response_header_bytes: usize,
    pub response_body_bytes: usize,
    pub response_raw_size: Option<usize>,
    pub response_decoded_size: Option<usize>,
}

pub fn sizes(s: &SessionRecord) -> Sizes {
    let req_line = format!("{} {} {}", s.method, s.path(), s.http_version);
    let resp_line = format!("{} {}", s.response_http_version, s.status);
    Sizes {
        request_header_bytes: header_bytes(&req_line, &s.request_headers),
        request_body_bytes: s.request_body.len(),
        response_header_bytes: header_bytes(&resp_line, &s.response_headers),
        response_body_bytes: s.response_body.len(),
        response_raw_size: s.response_raw_size,
        response_decoded_size: s.response_decoded_size,
    }
}

/// One phase of the Timings waterfall: (label, start offset ms, length ms).
pub type Phase = (&'static str, u64, u64);

/// Timing phases laid out one after another. Missing phases are skipped; "Receive" is whatever is
/// left of `duration_ms` after the known phases.
pub fn timing_phases(s: &SessionRecord) -> Vec<Phase> {
    let mut out = Vec::new();
    let mut at = 0u64;
    for (label, v) in [("Request sent", s.send_ms), ("DNS", s.dns_ms), ("TCP connect", s.connect_ms), ("TLS handshake", s.tls_ms), ("Server (TTFB)", s.ttfb_ms)] {
        if let Some(v) = v {
            out.push((label, at, v));
            at += v;
        }
    }
    let total = s.duration_ms as u64;
    if total > at {
        out.push(("Receive", at, total - at));
    }
    out
}

/// Default folder for "Save body": `FERROTAP_DOWNLOADS_DIR` if set, else `%USERPROFILE%\Downloads`
/// (or `$HOME/Downloads`), else the current directory.
pub fn downloads_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FERROTAP_DOWNLOADS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    for var in ["USERPROFILE", "HOME"] {
        if let Some(h) = std::env::var_os(var).filter(|d| !d.is_empty()) {
            return PathBuf::from(h).join("Downloads");
        }
    }
    PathBuf::from(".")
}

/// A file name for a body: last URL path segment (sanitised), with an extension from the content type when missing.
pub fn suggested_file_name(url: &str, content_type: &str, id: usize, side: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path = path.split_once("://").map(|x| x.1).unwrap_or(path);
    let last = path.split('/').skip(1).filter(|p| !p.is_empty()).last().unwrap_or("");
    let mut name: String = last.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    if name.is_empty() || name.starts_with('.') {
        name = format!("session{id}_{side}");
    }
    if !name.contains('.') {
        let ct = content_type.to_ascii_lowercase();
        let ext = if ct.contains("json") {
            "json"
        } else if ct.contains("html") {
            "html"
        } else if ct.contains("xml") {
            "xml"
        } else if ct.contains("javascript") {
            "js"
        } else if ct.contains("css") {
            "css"
        } else {
            "txt"
        };
        name = format!("{name}.{ext}");
    }
    name
}

/// Write `text` to `path` (parent folders are created). Returns the number of bytes written.
pub fn save_text(path: &std::path::Path, text: &str) -> Result<usize, String> {
    if path.as_os_str().is_empty() {
        return Err("No file path given".into());
    }
    if let Some(p) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(p).map_err(|e| format!("Cannot create {}: {e}", p.display()))?;
    }
    std::fs::write(path, text.as_bytes()).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    Ok(text.len())
}

// ---------------------------------------------------------------------------------------------
// UI state and egui helpers
// ---------------------------------------------------------------------------------------------

/// Per-side (request or response) body view state.
#[derive(Default)]
pub struct BodyUi {
    pub search: String,
    pub current: usize,
    pub scroll_pending: bool,
    pub show_original: bool,
    pub save_path: String,
    pub save_msg: String,
    pub save_for: Option<(usize, String)>,
}

/// This module's UI state (kept in `GridApp::ext.inspect`).
pub struct State {
    pub req: BodyUi,
    pub resp: BodyUi,
    pub wrap: bool,
    pub decode_open: bool,
    pub decode_input: String,
    pub decode_output: String,
    pub decode_op: DecodeOp,
    /// Image tab (decoded texture cache) for the response and request sides.
    pub image: crate::imageview::ImageState,
    pub req_image: crate::imageview::ImageState,
    /// Response bytes of the shown session (decoding base64 every frame is slow for big bodies).
    pub resp_bytes: Option<((usize, usize, usize), std::sync::Arc<Vec<u8>>)>,
    /// Last formatted body (JSON / XML / JS pretty-printing is slow for big bodies).
    pub formatted: Option<(FormatKey, std::sync::Arc<String>)>,
    /// The same two caches for the request side.
    pub req_bytes: Option<((usize, usize, usize), std::sync::Arc<Vec<u8>>)>,
    pub formatted_req: Option<(FormatKey, std::sync::Arc<String>)>,
}

/// What a formatted body was made from: session, tab, length and a hash of its start.
pub type FormatKey = (usize, u8, usize, u64);

impl Default for State {
    fn default() -> Self {
        State {
            req: BodyUi::default(),
            resp: BodyUi::default(),
            wrap: true,
            decode_open: false,
            decode_input: String::new(),
            decode_output: String::new(),
            decode_op: DecodeOp::Base64Decode,
            image: crate::imageview::ImageState { cached: None, fit: true },
            req_image: crate::imageview::ImageState { cached: None, fit: true },
            resp_bytes: None,
            formatted: None,
            req_bytes: None,
            formatted_req: None,
        }
    }
}

use eframe::egui::{self, Color32, RichText};

fn small_gray(ui: &mut egui::Ui, t: impl Into<String>) {
    ui.label(RichText::new(t.into()).small().color(Color32::GRAY));
}

/// Shows the Content-Encoding banner with a "decoded / original" toggle and returns the body to display.
pub fn encoding_bar<'a>(ui: &mut egui::Ui, b: &mut BodyUi, content_encoding: Option<&str>, content_type: Option<&str>, stored: &'a str) -> Cow<'a, str> {
    encoding_bar_raw(ui, b, content_encoding, content_type, stored, stored.as_bytes())
}

/// `encoding_bar` that decodes the exact captured bytes `raw` (the text `stored` is shown as "Original").
pub fn encoding_bar_raw<'a>(ui: &mut egui::Ui, b: &mut BodyUi, content_encoding: Option<&str>, content_type: Option<&str>, stored: &'a str, raw: &[u8]) -> Cow<'a, str> {
    match decode_body_bytes(raw, content_encoding, content_type) {
        // No Content-Encoding: still honour a non-UTF-8 charset on the raw bytes.
        BodyDecode::NotEncoded => match decode_charset(raw, content_type) {
            Some(t) => Cow::Owned(t),
            None => Cow::Borrowed(stored),
        },
        BodyDecode::Decoded { encodings, text } => {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("Content-Encoding: {}", content_encoding.unwrap_or(""))).small().color(Color32::LIGHT_GREEN));
                ui.selectable_value(&mut b.show_original, false, "Decoded");
                ui.selectable_value(&mut b.show_original, true, "Original");
                small_gray(ui, format!("undid {} ({} -> {} bytes)", encodings.join(", "), raw.len(), text.len()));
            });
            if b.show_original {
                Cow::Borrowed(stored)
            } else {
                Cow::Owned(text)
            }
        }
        BodyDecode::Failed { encodings, reason, already_decoded } => {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("Content-Encoding: {}", encodings.join(", "))).small().color(Color32::from_rgb(230, 160, 40)));
                if already_decoded {
                    small_gray(ui, "the stored body is already readable text (shown as stored)");
                } else {
                    small_gray(ui, format!("cannot decode: {reason}"));
                }
            });
            Cow::Borrowed(stored)
        }
    }
}

/// Body toolbar (word wrap, search with match count and next/previous, save to file) plus the
/// highlighted, read-only text view. `save` = (session id, suggested file name).
pub fn body_view(ui: &mut egui::Ui, b: &mut BodyUi, wrap: &mut bool, id_salt: &str, text: &str, save: (usize, String)) {
    let len = text.len();
    let mut end = len.min(500_000);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let shown = &text[..end];
    let matches = find_matches(shown, &b.search, false);
    if b.current >= matches.len() {
        b.current = 0;
    }
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(wrap, "Wrap");
        ui.separator();
        let r = ui.add(egui::TextEdit::singleline(&mut b.search).hint_text("search body").desired_width(140.0).id_salt(format!("{id_salt}_search")));
        if r.changed() {
            b.current = 0;
            b.scroll_pending = true;
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if !b.search.is_empty() {
            let n = matches.len();
            ui.label(RichText::new(if n == 0 { "0 matches".to_string() } else { format!("{}/{} matches", b.current + 1, n) }).small());
            if ui.small_button("<").on_hover_text("Previous match").clicked() && n > 0 {
                b.current = (b.current + n - 1) % n;
                b.scroll_pending = true;
            }
            if (ui.small_button(">").on_hover_text("Next match (Enter)").clicked() || enter) && n > 0 {
                b.current = (b.current + 1) % n;
                b.scroll_pending = true;
                if enter {
                    r.request_focus();
                }
            }
        }
        ui.separator();
        if b.save_for.as_ref() != Some(&save) {
            b.save_path = downloads_dir().join(&save.1).display().to_string();
            b.save_for = Some(save.clone());
            b.save_msg.clear();
        }
        ui.add(egui::TextEdit::singleline(&mut b.save_path).desired_width(200.0).id_salt(format!("{id_salt}_save")));
        if ui.small_button("Save body").on_hover_text("Write the body shown below to this file").clicked() {
            b.save_msg = match save_text(std::path::Path::new(b.save_path.trim()), text) {
                Ok(n) => format!("Saved {n} bytes"),
                Err(e) => e,
            };
        }
        if !b.save_msg.is_empty() {
            small_gray(ui, b.save_msg.clone());
        }
    });
    if len > end {
        small_gray(ui, format!("(showing the first {end} of {len} bytes)"));
    }
    let current = b.current;
    let wrap_on = *wrap;
    let font = egui::FontId::monospace(12.0);
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let s = buf.as_str();
        let mut job = egui::text::LayoutJob::default();
        let color = ui.visuals().text_color();
        let plain = egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
        let mut at = 0;
        for (i, m) in matches.iter().enumerate() {
            if m.end > s.len() || m.start < at {
                continue;
            }
            job.append(&s[at..m.start], 0.0, plain.clone());
            let bg = if i == current { Color32::from_rgb(255, 140, 0) } else { Color32::from_rgb(120, 110, 30) };
            job.append(&s[m.clone()], 0.0, egui::TextFormat { background: bg, color: Color32::WHITE, ..plain.clone() });
            at = m.end;
        }
        job.append(&s[at..], 0.0, plain);
        job.wrap.max_width = if wrap_on { wrap_width } else { f32::INFINITY };
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let scroll = if wrap_on { egui::ScrollArea::vertical() } else { egui::ScrollArea::both() };
    scroll.id_salt(id_salt).auto_shrink([false, false]).show(ui, |ui| {
        let mut t: &str = shown;
        let out = egui::TextEdit::multiline(&mut t)
            .id_salt(format!("{id_salt}_te"))
            .desired_width(if wrap_on { ui.available_width() } else { f32::INFINITY })
            .desired_rows(12)
            .layouter(&mut layouter)
            .show(ui);
        if b.scroll_pending {
            b.scroll_pending = false;
            if let Some(m) = matches.get(current) {
                let ci = shown[..m.start].chars().count();
                let r = out.galley.pos_from_cursor(egui::text::CCursor::new(ci)).translate(out.galley_pos.to_vec2());
                ui.scroll_to_rect(r.expand(20.0), Some(egui::Align::Center));
            }
        }
    });
}

/// Key/value grid with a right-click menu on every cell: copy name / value / both.
pub fn kv_grid(ui: &mut egui::Ui, id: &str, headers: (&str, &str), rows: &[(String, String)]) {
    egui::Grid::new(id).striped(true).num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new(headers.0).strong());
        ui.label(RichText::new(headers.1).strong());
        ui.end_row();
        for (k, v) in rows {
            let a = ui.add(egui::Label::new(RichText::new(k).monospace().strong()).selectable(false).sense(egui::Sense::click()));
            let b = ui.add(egui::Label::new(RichText::new(v).monospace()).selectable(false).sense(egui::Sense::click()));
            for r in [a, b] {
                r.on_hover_text("Right-click to copy").context_menu(|ui| copy_menu(ui, k, v));
            }
            ui.end_row();
        }
    });
}

/// The copy menu used by header/param/cookie rows.
pub fn copy_menu(ui: &mut egui::Ui, k: &str, v: &str) {
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(k.to_string());
        ui.close();
    }
    if ui.button("Copy value").clicked() {
        ui.ctx().copy_text(v.to_string());
        ui.close();
    }
    if ui.button("Copy name: value").clicked() {
        ui.ctx().copy_text(format!("{k}: {v}"));
        ui.close();
    }
}

/// Query string parameters table.
pub fn params_view(ui: &mut egui::Ui, s: &SessionRecord) {
    let q = parse_query(&s.url);
    if q.is_empty() {
        small_gray(ui, "(No query string parameters)");
        return;
    }
    egui::ScrollArea::both().id_salt("insp_params").auto_shrink([false, false]).show(ui, |ui| {
        kv_grid(ui, "grid_params", ("Name", "Value (decoded)"), &q);
    });
}

/// Request Cookie pairs (`response == false`) or the response Set-Cookie table.
pub fn cookies_view(ui: &mut egui::Ui, s: &SessionRecord, response: bool) {
    let salt = if response { "insp_cookies_resp" } else { "insp_cookies_req" };
    egui::ScrollArea::both().id_salt(salt).auto_shrink([false, false]).show(ui, |ui| {
        if !response {
            let pairs: Vec<(String, String)> = s
                .request_headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
                .flat_map(|(_, v)| parse_cookie_header(v))
                .collect();
            if pairs.is_empty() {
                small_gray(ui, "(No Cookie header)");
            } else {
                kv_grid(ui, "grid_req_cookies", ("Cookie", "Value"), &pairs);
            }
            return;
        }
        let cookies = response_set_cookies(&s.response_headers);
        if cookies.is_empty() {
            small_gray(ui, "(No Set-Cookie header)");
            return;
        }
        egui::Grid::new("grid_set_cookies").striped(true).num_columns(9).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Name", "Value", "Domain", "Path", "Expires", "Max-Age", "HttpOnly", "Secure", "SameSite"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            let o = |x: &Option<String>| x.clone().unwrap_or_default();
            for c in &cookies {
                let r = ui.add(egui::Label::new(RichText::new(&c.name).monospace().strong()).selectable(false).sense(egui::Sense::click()));
                r.context_menu(|ui| copy_menu(ui, &c.name, &c.value));
                let r = ui.add(egui::Label::new(RichText::new(&c.value).monospace()).selectable(false).sense(egui::Sense::click()));
                r.context_menu(|ui| copy_menu(ui, &c.name, &c.value));
                ui.label(o(&c.domain));
                ui.label(o(&c.path));
                ui.label(o(&c.expires));
                ui.label(o(&c.max_age));
                ui.label(if c.http_only { "yes" } else { "" });
                ui.label(if c.secure { "yes" } else { "" });
                ui.label(o(&c.same_site));
                ui.end_row();
            }
        });
    });
}

/// Web forms: urlencoded fields or multipart parts.
pub fn forms_view(ui: &mut egui::Ui, s: &SessionRecord) {
    let Some(parts) = parse_form(s) else {
        small_gray(ui, "(Request body is not application/x-www-form-urlencoded or multipart/form-data)");
        return;
    };
    if parts.is_empty() {
        small_gray(ui, "(Form has no fields)");
        return;
    }
    egui::ScrollArea::both().id_salt("insp_forms").auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("grid_forms").striped(true).num_columns(5).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Name", "Value", "File name", "Content-Type", "Size"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for p in &parts {
                let shown: String = if p.value.is_empty() && p.size > 0 { "(file data)".into() } else { p.value.chars().take(200).collect() };
                let r = ui.add(egui::Label::new(RichText::new(&p.name).monospace().strong()).selectable(false).sense(egui::Sense::click()));
                r.context_menu(|ui| copy_menu(ui, &p.name, &p.value));
                ui.label(RichText::new(shown).monospace());
                ui.label(p.filename.clone().unwrap_or_default());
                ui.label(p.content_type.clone().unwrap_or_default());
                ui.label(format!("{} B", p.size));
                ui.end_row();
            }
        });
    });
}

/// Response trailers table.
pub fn trailers_view(ui: &mut egui::Ui, s: &SessionRecord) {
    if s.response_trailers.is_empty() {
        small_gray(ui, "(No trailers)");
        return;
    }
    kv_grid(ui, "grid_trailers", ("Trailer", "Value"), &s.response_trailers);
}

/// Timings waterfall: one bar per phase, offset by the phases before it.
pub fn timings_view(ui: &mut egui::Ui, s: &SessionRecord) {
    let phases = timing_phases(s);
    let total = (s.duration_ms as u64).max(phases.last().map(|p| p.1 + p.2).unwrap_or(0)).max(1);
    let colors = [
        Color32::from_rgb(90, 160, 220),
        Color32::from_rgb(240, 170, 60),
        Color32::from_rgb(170, 110, 220),
        Color32::from_rgb(80, 180, 110),
        Color32::from_rgb(100, 140, 170),
    ];
    egui::Grid::new("grid_timings").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
        for (i, (label, start, len)) in phases.iter().enumerate() {
            ui.label(*label);
            ui.label(RichText::new(format!("{len} ms")).monospace());
            let w = 260.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, ui.visuals().faint_bg_color);
            let x0 = rect.left() + w * (*start as f32 / total as f32);
            let x1 = (x0 + (w * (*len as f32 / total as f32)).max(2.0)).min(rect.right());
            let bar = egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom()));
            ui.painter().rect_filled(bar, 2.0, colors[i % colors.len()]);
            ui.end_row();
        }
        ui.label(RichText::new("Total").strong());
        ui.label(RichText::new(format!("{} ms", s.duration_ms)).monospace().strong());
        ui.end_row();
    });
    let missing: Vec<&str> = [("DNS", s.dns_ms), ("TCP connect", s.connect_ms), ("TLS", s.tls_ms), ("TTFB", s.ttfb_ms)]
        .iter()
        .filter(|(_, v)| v.is_none())
        .map(|(l, _)| *l)
        .collect();
    if !missing.is_empty() {
        small_gray(ui, format!("Not measured for this session: {} (e.g. reused connection or plain HTTP)", missing.join(", ")));
    }
}

/// Sizes table.
pub fn sizes_view(ui: &mut egui::Ui, s: &SessionRecord) {
    let z = sizes(s);
    let opt = |o: Option<usize>| o.map(|n| format!("{n} B")).unwrap_or_else(|| "-".into());
    egui::Grid::new("grid_sizes").striped(true).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        let total = z.request_header_bytes + z.request_body_bytes + z.response_header_bytes + z.response_raw_size.unwrap_or(z.response_body_bytes);
        let rows = [
            ("Request headers", format!("{} B", z.request_header_bytes)),
            ("Request body", format!("{} B", z.request_body_bytes)),
            ("Response headers", format!("{} B", z.response_header_bytes)),
            ("Response body (stored)", format!("{} B", z.response_body_bytes)),
            ("Response body on the wire (raw)", opt(z.response_raw_size)),
            ("Response body decoded", opt(z.response_decoded_size)),
            ("Total transferred (approx.)", format!("{total} B")),
        ];
        for (k, v) in rows {
            ui.label(k);
            ui.label(RichText::new(v).monospace());
            ui.end_row();
        }
        if let (Some(r), Some(d)) = (z.response_raw_size, z.response_decoded_size) {
            if d > 0 && r != d {
                ui.label("Compressed / decoded");
                ui.label(RichText::new(format!("{:.1}%", r as f64 * 100.0 / d as f64)).monospace());
                ui.end_row();
            }
        }
    });
}

impl GridApp {
    /// Toolbar entry (a button or a menu) for this module.
    pub fn inspect_menu(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(self.ext.inspect.decode_open, "Decode...")
            .on_hover_text("Base64 / URL / HTML entities / hex / JWT decoder")
            .clicked()
        {
            self.ext.inspect.decode_open = !self.ext.inspect.decode_open;
        }
    }

    /// Windows this module draws each frame.
    pub fn inspect_windows(&mut self, ctx: &egui::Context) {
        if !self.ext.inspect.decode_open {
            return;
        }
        let sel = self.model.selected_row().map(|s| {
            (s.request_body.clone(), s.response_body.clone(), s.request_header("authorization").map(str::to_string))
        });
        let st = &mut self.ext.inspect;
        let mut open = true;
        egui::Window::new("Decode tool").open(&mut open).resizable(true).default_width(560.0).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Input");
                match &sel {
                    Some((req, resp, auth)) => {
                        if ui.small_button("Use request body").clicked() {
                            st.decode_input = req.clone();
                        }
                        if ui.small_button("Use response body").clicked() {
                            st.decode_input = resp.clone();
                        }
                        if let Some(a) = auth {
                            if ui.small_button("Use Authorization header").clicked() {
                                st.decode_input = a.clone();
                            }
                        }
                    }
                    None => small_gray(ui, "(select a session to use its bodies)"),
                }
                if ui.small_button("Clear").clicked() {
                    st.decode_input.clear();
                    st.decode_output.clear();
                }
            });
            egui::ScrollArea::vertical().id_salt("dec_in").max_height(140.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut st.decode_input)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(5)
                        .hint_text("paste text here"),
                );
            });
            ui.horizontal_wrapped(|ui| {
                for op in DecodeOp::ALL {
                    if ui.button(op.label()).clicked() {
                        st.decode_op = op;
                        st.decode_output = match run_decode(op, &st.decode_input) {
                            Ok(s) => s,
                            Err(e) => format!("Error: {e}"),
                        };
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Output ({})", st.decode_op.label())).strong());
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(st.decode_output.clone());
                }
                if ui.small_button("Use as input").clicked() {
                    st.decode_input = st.decode_output.clone();
                }
            });
            egui::ScrollArea::both().id_salt("dec_out").auto_shrink([false, false]).max_height(260.0).show(ui, |ui| {
                let mut t: &str = &st.decode_output;
                ui.add(egui::TextEdit::multiline(&mut t).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(8));
            });
        });
        if !open {
            self.ext.inspect.decode_open = false;
        }
    }
}

/// The Hex tab shows at most this many bytes (a hex dump is ~4.5x the body size).
pub const HEX_MAX: usize = 64 * 1024;

/// A classic hex dump: offset, 16 bytes in hex (two groups of 8), then the printable ASCII.
pub fn hex_dump(bytes: &[u8], max: usize) -> String {
    let shown = &bytes[..bytes.len().min(max)];
    let mut out = String::with_capacity(shown.len() * 5);
    for (n, chunk) in shown.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", n * 16));
        for k in 0..16 {
            match chunk.get(k) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if k == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(chunk.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }));
        out.push('\n');
    }
    if bytes.len() > max {
        out.push_str(&format!("... {} more bytes not shown\n", bytes.len() - max));
    }
    if bytes.is_empty() {
        out.push_str("(empty body)\n");
    }
    out
}

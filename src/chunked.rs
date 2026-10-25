//! Stage 1 of the response-body decode pipeline: HTTP/1.1 `Transfer-Encoding: chunked` reassembly.
//!
//! Pure, side-effect-free, no I/O and no UI (same rule as `model.rs`), so it is unit-testable.
//! Trailers (e.g. `Signature:`) are captured into [`ChunkedBody::trailers`] instead of being dropped.
//!
//! Gate: only HTTP/1.1 messages whose *final* transfer coding is `chunked` are de-chunked
//! ([`chunked_applies`]). HTTP/1.0, HTTP/2 and HTTP/3 pass through untouched: h2/h3 have no
//! Transfer-Encoding and deliver DATA frames already reassembled by the existing stream tracking.
//!
//! Next stage (Content-Encoding gzip/deflate/br/zstd) consumes `ChunkedBody::data`; it must grow its
//! output buffer dynamically and never size it from Content-Length.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Maximum length of a chunk-size line or a trailer line (bytes, excluding CRLF).
pub const MAX_LINE: usize = 8192;
/// Maximum number of trailer fields accepted.
pub const MAX_TRAILERS: usize = 64;
/// Default cap on the reassembled body (1 GiB).
pub const DEFAULT_MAX_BODY: usize = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpVersion {
    Http10,
    Http11,
    H2,
    H3,
}

impl HttpVersion {
    /// Parses `HTTP/1.1`, `HTTP/2`, `HTTP/2.0`, `h2`, `h3`, `1.1`, ... (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "HTTP/1.0" | "1.0" => Some(Self::Http10),
            "HTTP/1.1" | "1.1" => Some(Self::Http11),
            "HTTP/2" | "HTTP/2.0" | "H2" | "2" | "2.0" => Some(Self::H2),
            "HTTP/3" | "HTTP/3.0" | "H3" | "3" | "3.0" => Some(Self::H3),
            _ => None,
        }
    }
}

/// True only for HTTP/1.1 with `chunked` as the last transfer coding (RFC 9112 §6.1).
pub fn chunked_applies(version: HttpVersion, transfer_encoding: Option<&str>) -> bool {
    if version != HttpVersion::Http11 {
        return false;
    }
    match transfer_encoding {
        Some(te) => te
            .split(',')
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .next_back()
            .is_some_and(|t| t == "chunked"),
        None => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trailer {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkExtension {
    /// Zero-based index of the chunk the extension was attached to (the last chunk is numbered after the data chunks).
    pub chunk_index: usize,
    pub name: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkError {
    BadChunkSize(String),
    MissingChunkCrlf,
    BareLf,
    LineTooLong,
    BadTrailer(String),
    TooManyTrailers,
    BodyTooLarge,
    Incomplete { state: &'static str, decoded: usize },
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadChunkSize(s) => write!(f, "invalid chunk size line: {s:?}"),
            Self::MissingChunkCrlf => write!(f, "chunk data not followed by CRLF"),
            Self::BareLf => write!(f, "line ended with bare LF (CRLF required)"),
            Self::LineTooLong => write!(f, "chunk/trailer line exceeds {MAX_LINE} bytes"),
            Self::BadTrailer(s) => write!(f, "invalid trailer line: {s:?}"),
            Self::TooManyTrailers => write!(f, "more than {MAX_TRAILERS} trailer fields"),
            Self::BodyTooLarge => write!(f, "decoded body exceeds size limit"),
            Self::Incomplete { state, decoded } => {
                write!(f, "incomplete chunked body (in {state}, {decoded} bytes decoded)")
            }
        }
    }
}

impl std::error::Error for ChunkError {}

/// Result of stage 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkedBody {
    /// Reassembled body (chunk framing removed). Unchanged input when `applied` is false.
    pub data: Vec<u8>,
    pub trailers: Vec<Trailer>,
    pub extensions: Vec<ChunkExtension>,
    /// Bytes of wire input that belong to this body (framing included) -> Inspector "raw size".
    pub raw_len: usize,
    /// Bytes after the terminating CRLF (pipelined data), not part of this body.
    pub leftover: Vec<u8>,
    /// False when the gate skipped de-chunking.
    pub applied: bool,
    pub skip_reason: Option<&'static str>,
}

impl ChunkedBody {
    /// Case-insensitive trailer lookup (first match).
    pub fn trailer(&self, name: &str) -> Option<&str> {
        self.trailers
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.value.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Size,
    Data(usize),
    DataEnd,
    Trailers,
    Done,
}

/// Incremental decoder: call [`feed`](Self::feed) with arbitrary slices as they arrive.
pub struct ChunkedDecoder {
    state: State,
    line: Vec<u8>,
    body: Vec<u8>,
    decoded_total: usize,
    trailers: Vec<Trailer>,
    extensions: Vec<ChunkExtension>,
    leftover: Vec<u8>,
    consumed: usize,
    chunks: usize,
    max_body: usize,
    failed: Option<ChunkError>,
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkedDecoder {
    pub fn new() -> Self {
        Self {
            state: State::Size,
            line: Vec::new(),
            body: Vec::new(),
            decoded_total: 0,
            trailers: Vec::new(),
            extensions: Vec::new(),
            leftover: Vec::new(),
            consumed: 0,
            chunks: 0,
            max_body: DEFAULT_MAX_BODY,
            failed: None,
        }
    }

    pub fn with_max_body(mut self, max: usize) -> Self {
        self.max_body = max;
        self
    }

    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Decoded bytes announced so far (including any already drained).
    pub fn decoded_len(&self) -> usize {
        self.decoded_total
    }

    /// Takes the decoded bytes accumulated so far (streaming use). `finish` then returns only the rest.
    pub fn drain_body(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }

    fn state_name(&self) -> &'static str {
        match self.state {
            State::Size => "chunk-size line",
            State::Data(_) => "chunk data",
            State::DataEnd => "chunk CRLF",
            State::Trailers => "trailers",
            State::Done => "done",
        }
    }

    /// Feeds more wire bytes. After an error the decoder stays failed.
    pub fn feed(&mut self, input: &[u8]) -> Result<(), ChunkError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let mut data = input;
        let r = self.feed_inner(&mut data);
        if let Err(e) = &r {
            self.failed = Some(e.clone());
        }
        r
    }

    fn feed_inner(&mut self, data: &mut &[u8]) -> Result<(), ChunkError> {
        loop {
            if self.state == State::Done {
                self.leftover.extend_from_slice(data);
                *data = &[];
                return Ok(());
            }
            if data.is_empty() {
                return Ok(());
            }
            match self.state {
                State::Size => {
                    if let Some(line) = self.read_line(data)? {
                        let n = self.parse_size_line(&line)?;
                        self.chunks += 1;
                        self.state = if n == 0 { State::Trailers } else { State::Data(n) };
                    }
                }
                State::Data(rem) => {
                    let n = rem.min(data.len());
                    self.body.extend_from_slice(&data[..n]);
                    self.consumed += n;
                    *data = &data[n..];
                    self.state = if rem == n { State::DataEnd } else { State::Data(rem - n) };
                }
                State::DataEnd => {
                    let b = data[0];
                    *data = &data[1..];
                    self.consumed += 1;
                    match (self.line.len(), b) {
                        (0, b'\r') => self.line.push(b),
                        (1, b'\n') => {
                            self.line.clear();
                            self.state = State::Size;
                        }
                        _ => return Err(ChunkError::MissingChunkCrlf),
                    }
                }
                State::Trailers => {
                    if let Some(line) = self.read_line(data)? {
                        if line.is_empty() {
                            self.state = State::Done;
                        } else {
                            self.parse_trailer(&line)?;
                        }
                    }
                }
                State::Done => unreachable!(),
            }
        }
    }

    /// Reads one CRLF-terminated line; `Ok(None)` means more input is needed.
    fn read_line(&mut self, data: &mut &[u8]) -> Result<Option<Vec<u8>>, ChunkError> {
        while let Some((&b, rest)) = data.split_first() {
            *data = rest;
            self.consumed += 1;
            if b == b'\n' {
                if self.line.last() != Some(&b'\r') {
                    return Err(ChunkError::BareLf);
                }
                self.line.pop();
                return Ok(Some(std::mem::take(&mut self.line)));
            }
            if self.line.len() >= MAX_LINE {
                return Err(ChunkError::LineTooLong);
            }
            self.line.push(b);
        }
        Ok(None)
    }

    fn parse_size_line(&mut self, line: &[u8]) -> Result<usize, ChunkError> {
        let s = String::from_utf8_lossy(line).into_owned();
        let bad = || ChunkError::BadChunkSize(s.clone());
        let (size_s, ext_s) = match s.find(';') {
            Some(i) => (&s[..i], Some(&s[i + 1..])),
            None => (s.as_str(), None),
        };
        let size_s = size_s.trim_matches(|c| c == ' ' || c == '\t');
        if size_s.is_empty() || size_s.len() > 16 || !size_s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let n = u64::from_str_radix(size_s, 16).map_err(|_| bad())?;
        let n = usize::try_from(n).map_err(|_| ChunkError::BodyTooLarge)?;
        if self.decoded_total.checked_add(n).is_none_or(|t| t > self.max_body) {
            return Err(ChunkError::BodyTooLarge);
        }
        self.decoded_total += n;
        if let Some(e) = ext_s {
            for part in split_quoted(e, ';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let (name, value) = match part.split_once('=') {
                    Some((k, v)) => (k.trim(), Some(unquote(v.trim()))),
                    None => (part, None),
                };
                if name.is_empty() {
                    return Err(bad());
                }
                self.extensions.push(ChunkExtension {
                    chunk_index: self.chunks,
                    name: name.to_string(),
                    value,
                });
            }
        }
        Ok(n)
    }

    fn parse_trailer(&mut self, line: &[u8]) -> Result<(), ChunkError> {
        let s = String::from_utf8_lossy(line).into_owned();
        if line[0] == b' ' || line[0] == b'\t' {
            return Err(ChunkError::BadTrailer(s));
        }
        let Some((name, value)) = s.split_once(':') else {
            return Err(ChunkError::BadTrailer(s));
        };
        if name.is_empty() || !name.bytes().all(is_tchar) {
            return Err(ChunkError::BadTrailer(s));
        }
        if self.trailers.len() >= MAX_TRAILERS {
            return Err(ChunkError::TooManyTrailers);
        }
        self.trailers.push(Trailer {
            name: name.to_string(),
            value: value.trim_matches(|c| c == ' ' || c == '\t').to_string(),
        });
        Ok(())
    }

    /// Completes decoding. Errors with `Incomplete` if the last chunk + trailer section was not seen.
    pub fn finish(self) -> Result<ChunkedBody, ChunkError> {
        if let Some(e) = self.failed.clone() {
            return Err(e);
        }
        if self.state != State::Done {
            return Err(ChunkError::Incomplete {
                state: self.state_name(),
                decoded: self.decoded_total,
            });
        }
        Ok(ChunkedBody {
            data: self.body,
            trailers: self.trailers,
            extensions: self.extensions,
            raw_len: self.consumed,
            leftover: self.leftover,
            applied: true,
            skip_reason: None,
        })
    }
}

/// One-shot stage-1 entry point with the version gate.
///
/// `raw` is the complete message body as read from the wire. For HTTP/2, HTTP/3, HTTP/1.0 or
/// non-chunked HTTP/1.1 the bytes are returned unchanged with `applied == false`.
pub fn decode_transfer(
    version: HttpVersion,
    transfer_encoding: Option<&str>,
    raw: &[u8],
) -> Result<ChunkedBody, ChunkError> {
    if !chunked_applies(version, transfer_encoding) {
        let reason = match version {
            HttpVersion::H2 | HttpVersion::H3 => "HTTP/2+ has no Transfer-Encoding (DATA frames pre-reassembled)",
            HttpVersion::Http10 => "HTTP/1.0 has no chunked encoding",
            HttpVersion::Http11 => "Transfer-Encoding is not chunked",
        };
        return Ok(ChunkedBody {
            data: raw.to_vec(),
            trailers: Vec::new(),
            extensions: Vec::new(),
            raw_len: raw.len(),
            leftover: Vec::new(),
            applied: false,
            skip_reason: Some(reason),
        });
    }
    let mut d = ChunkedDecoder::new();
    d.feed(raw)?;
    d.finish()
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// Splits on `sep` outside double-quoted strings.
fn split_quoted(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut in_q, mut esc) = (0, false, false);
    for (i, c) in s.char_indices() {
        if esc {
            esc = false;
        } else if in_q && c == '\\' {
            esc = true;
        } else if c == '"' {
            in_q = !in_q;
        } else if c == sep && !in_q {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::new();
        let mut esc = false;
        for c in inner.chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        out
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H11: HttpVersion = HttpVersion::Http11;
    const TE: Option<&str> = Some("chunked");

    #[test]
    fn simple_body() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let b = decode_transfer(H11, TE, raw).unwrap();
        assert!(b.applied);
        assert_eq!(b.data, b"hello world");
        assert!(b.trailers.is_empty());
        assert_eq!(b.raw_len, raw.len());
    }

    #[test]
    fn hex_sizes_upper_and_lower() {
        let raw = b"A\r\n0123456789\r\nb\r\n0123456789a\r\n0\r\n\r\n";
        let b = decode_transfer(H11, TE, raw).unwrap();
        assert_eq!(b.data.len(), 21);
    }

    #[test]
    fn trailers_captured() {
        let raw = b"3\r\nabc\r\n0\r\nSignature: sig=:YWJj:\r\nX-Checksum:  42 \r\n\r\n";
        let b = decode_transfer(H11, TE, raw).unwrap();
        assert_eq!(b.data, b"abc");
        assert_eq!(b.trailers.len(), 2);
        assert_eq!(b.trailer("signature"), Some("sig=:YWJj:"));
        assert_eq!(b.trailer("X-CHECKSUM"), Some("42"));
        assert_eq!(b.raw_len, raw.len());
    }

    #[test]
    fn extensions_parsed() {
        let raw = b"3;foo=bar;q=\"a;b\"\r\nabc\r\n0;done\r\n\r\n";
        let b = decode_transfer(H11, TE, raw).unwrap();
        assert_eq!(b.data, b"abc");
        assert_eq!(b.extensions.len(), 3);
        assert_eq!(b.extensions[0].name, "foo");
        assert_eq!(b.extensions[0].value.as_deref(), Some("bar"));
        assert_eq!(b.extensions[1].value.as_deref(), Some("a;b"));
        assert_eq!(b.extensions[2].name, "done");
        assert_eq!(b.extensions[2].value, None);
    }

    #[test]
    fn byte_by_byte_equals_one_shot() {
        let raw = b"4;x=1\r\nWiki\r\n5\r\npedia\r\n0\r\nSignature: abc\r\n\r\n";
        let one = decode_transfer(H11, TE, raw).unwrap();
        let mut d = ChunkedDecoder::new();
        for byte in raw.iter() {
            assert!(!d.is_done());
            d.feed(std::slice::from_ref(byte)).unwrap();
        }
        assert!(d.is_done());
        assert_eq!(d.finish().unwrap(), one);
        assert_eq!(one.data, b"Wikipedia");
    }

    #[test]
    fn streaming_drain() {
        let mut d = ChunkedDecoder::new();
        d.feed(b"3\r\nabc\r\n2\r\nd").unwrap();
        assert_eq!(d.drain_body(), b"abcd");
        d.feed(b"e\r\n0\r\n\r\n").unwrap();
        assert_eq!(d.decoded_len(), 5);
        let b = d.finish().unwrap();
        assert_eq!(b.data, b"e");
    }

    #[test]
    fn leftover_pipelined_bytes() {
        let raw = b"1\r\nx\r\n0\r\n\r\nHTTP/1.1 200 OK\r\n";
        let b = decode_transfer(H11, TE, raw).unwrap();
        assert_eq!(b.data, b"x");
        assert_eq!(b.leftover, b"HTTP/1.1 200 OK\r\n");
        assert_eq!(b.raw_len, raw.len() - b.leftover.len());
    }

    #[test]
    fn binary_data_containing_crlf() {
        let raw = b"6\r\na\r\n0\r\n\r\n0\r\n\r\n";
        let b = decode_transfer(H11, TE, raw).unwrap();
        assert_eq!(b.data, b"a\r\n0\r\n");
    }

    #[test]
    fn malformed_bad_hex() {
        assert!(matches!(
            decode_transfer(H11, TE, b"zz\r\nab\r\n0\r\n\r\n"),
            Err(ChunkError::BadChunkSize(_))
        ));
        assert!(matches!(
            decode_transfer(H11, TE, b"\r\n0\r\n\r\n"),
            Err(ChunkError::BadChunkSize(_))
        ));
        assert!(matches!(
            decode_transfer(H11, TE, b"-1\r\n"),
            Err(ChunkError::BadChunkSize(_))
        ));
        assert!(matches!(
            decode_transfer(H11, TE, b"0x5\r\nhello\r\n"),
            Err(ChunkError::BadChunkSize(_))
        ));
    }

    #[test]
    fn malformed_size_overflow() {
        assert!(matches!(
            decode_transfer(H11, TE, b"FFFFFFFFFFFFFFFFF\r\n"),
            Err(ChunkError::BadChunkSize(_))
        ));
        assert_eq!(
            decode_transfer(H11, TE, b"FFFFFFFFFFFFFFFF\r\n").unwrap_err(),
            ChunkError::BodyTooLarge
        );
    }

    #[test]
    fn malformed_missing_crlf_after_data() {
        assert_eq!(
            decode_transfer(H11, TE, b"3\r\nabcde\r\n0\r\n\r\n").unwrap_err(),
            ChunkError::MissingChunkCrlf
        );
    }

    #[test]
    fn malformed_bare_lf() {
        assert_eq!(
            decode_transfer(H11, TE, b"3\nabc\r\n0\r\n\r\n").unwrap_err(),
            ChunkError::BareLf
        );
    }

    #[test]
    fn malformed_truncated() {
        let e = decode_transfer(H11, TE, b"5\r\nhel").unwrap_err();
        assert!(matches!(e, ChunkError::Incomplete { state: "chunk data", .. }));
        let e = decode_transfer(H11, TE, b"3\r\nabc\r\n0\r\nSignature: x\r\n").unwrap_err();
        assert!(matches!(e, ChunkError::Incomplete { state: "trailers", decoded: 3 }));
        assert!(matches!(
            decode_transfer(H11, TE, b"").unwrap_err(),
            ChunkError::Incomplete { .. }
        ));
    }

    #[test]
    fn malformed_trailers() {
        assert!(matches!(
            decode_transfer(H11, TE, b"0\r\nno colon here\r\n\r\n"),
            Err(ChunkError::BadTrailer(_))
        ));
        assert!(matches!(
            decode_transfer(H11, TE, b"0\r\nA: b\r\n folded\r\n\r\n"),
            Err(ChunkError::BadTrailer(_))
        ));
        assert!(matches!(
            decode_transfer(H11, TE, b"0\r\nBad Name: v\r\n\r\n"),
            Err(ChunkError::BadTrailer(_))
        ));
        let mut raw = b"0\r\n".to_vec();
        for i in 0..=MAX_TRAILERS {
            raw.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        assert_eq!(decode_transfer(H11, TE, &raw).unwrap_err(), ChunkError::TooManyTrailers);
    }

    #[test]
    fn malformed_long_line_and_limit() {
        let mut raw = vec![b'1'; MAX_LINE + 10];
        raw.extend_from_slice(b"\r\n");
        assert_eq!(decode_transfer(H11, TE, &raw).unwrap_err(), ChunkError::LineTooLong);
        let mut d = ChunkedDecoder::new().with_max_body(4);
        assert_eq!(d.feed(b"5\r\nhello\r\n").unwrap_err(), ChunkError::BodyTooLarge);
        // decoder stays failed
        assert_eq!(d.feed(b"0\r\n\r\n").unwrap_err(), ChunkError::BodyTooLarge);
    }

    #[test]
    fn version_gate_skips_h2_h3_http10() {
        let raw = b"5\r\nhello\r\n0\r\n\r\n";
        for v in [HttpVersion::H2, HttpVersion::H3, HttpVersion::Http10] {
            let b = decode_transfer(v, TE, raw).unwrap();
            assert!(!b.applied, "{v:?}");
            assert_eq!(b.data, raw.to_vec());
            assert!(b.trailers.is_empty());
            assert!(b.skip_reason.is_some());
        }
        // Even clearly malformed framing is not touched when the gate skips it.
        assert!(decode_transfer(HttpVersion::H2, TE, b"zz\r\n").is_ok());
    }

    #[test]
    fn version_gate_requires_chunked_last() {
        assert!(chunked_applies(H11, Some("chunked")));
        assert!(chunked_applies(H11, Some("Gzip, Chunked")));
        assert!(!chunked_applies(H11, Some("chunked, gzip")));
        assert!(!chunked_applies(H11, Some("identity")));
        assert!(!chunked_applies(H11, None));
        let b = decode_transfer(H11, None, b"plain").unwrap();
        assert!(!b.applied);
        assert_eq!(b.data, b"plain");
    }

    #[test]
    fn version_parse() {
        assert_eq!(HttpVersion::parse("HTTP/1.1"), Some(HttpVersion::Http11));
        assert_eq!(HttpVersion::parse("http/2.0"), Some(HttpVersion::H2));
        assert_eq!(HttpVersion::parse("h3"), Some(HttpVersion::H3));
        assert_eq!(HttpVersion::parse("HTTP/1.0"), Some(HttpVersion::Http10));
        assert_eq!(HttpVersion::parse("SPDY/3"), None);
    }
}

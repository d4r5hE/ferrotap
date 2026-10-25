//! Stage 2 of the response-body decode pipeline: Content-Encoding decompression and viewer payload prep.
//!
//! Handles gzip, deflate (zlib/raw deflate), br (brotli), and zstd, including stacked encodings
//! (e.g. "gzip, br" applied right-to-left / reverse order).
//! Decodes into a dynamically growable buffer (never sized off Content-Length, which reflects compressed size),
//! protected by a configurable decompression bomb limit (default 64 MiB).

use std::io::Read;

use crate::chunked::{decode_transfer, ChunkError, HttpVersion, Trailer};

/// Default maximum permitted decoded size to defend against decompression bombs (zip bombs / amplification attacks).
pub const DEFAULT_MAX_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Known content encodings supported by the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentCoding {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
    Identity,
}

impl ContentCoding {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => Some(Self::Gzip),
            "deflate" => Some(Self::Deflate),
            "br" => Some(Self::Brotli),
            "zstd" => Some(Self::Zstd),
            "identity" => Some(Self::Identity),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "br",
            Self::Zstd => "zstd",
            Self::Identity => "identity",
        }
    }
}

/// Errors that can occur during Content-Encoding decompression.
#[derive(Debug)]
pub enum DecodeError {
    DecompressionBomb {
        limit_bytes: usize,
        bytes_read: usize,
    },
    UnsupportedEncoding(String),
    MalformedData(String),
    ChunkError(ChunkError),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DecompressionBomb { limit_bytes, bytes_read } => {
                write!(f, "Decompression limit exceeded (read {} bytes, max permitted {})", bytes_read, limit_bytes)
            }
            Self::UnsupportedEncoding(enc) => write!(f, "Unsupported Content-Encoding: {}", enc),
            Self::MalformedData(msg) => write!(f, "Corrupted compressed stream: {}", msg),
            Self::ChunkError(err) => write!(f, "Chunk decode error: {:?}", err),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<ChunkError> for DecodeError {
    fn from(err: ChunkError) -> Self {
        Self::ChunkError(err)
    }
}

/// Clean payload handed to the R3 MIME viewer layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerPayload {
    /// Clean, decoded bytes (ready for JSON/XML/image parsing).
    pub body: Vec<u8>,
    /// Cleaned or inferred Content-Type (e.g. "application/json; charset=utf-8").
    pub content_type: Option<String>,
    /// Size in bytes before any chunked reassembly or decompression.
    pub raw_size: usize,
    /// Size in bytes after chunk reassembly and decompression.
    pub decoded_size: usize,
    /// Content encodings that were successfully stripped/decompressed (in execution order).
    pub encodings_applied: Vec<String>,
    /// Any HTTP trailers captured at the tail of chunked transfer (e.g. `Signature:`).
    pub trailers: Vec<Trailer>,
}

/// Configuration options for the decompression stage.
#[derive(Debug, Clone)]
pub struct DecoderLimits {
    pub max_decompressed_bytes: usize,
    pub initial_buffer_capacity: usize,
}

impl Default for DecoderLimits {
    fn default() -> Self {
        Self {
            max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
            initial_buffer_capacity: 8 * 1024, // 8 KiB growable start
        }
    }
}

/// Reads a stream up to `max_bytes` into a growable buffer, erroring if limit exceeded.
fn read_with_bomb_limit<R: Read>(mut reader: R, limits: &DecoderLimits) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::with_capacity(limits.initial_buffer_capacity);
    let mut chunk = [0u8; 16 * 1024];
    let mut total = 0usize;

    loop {
        let n = reader.read(&mut chunk).map_err(|e| DecodeError::MalformedData(e.to_string()))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        if total > limits.max_decompressed_bytes {
            return Err(DecodeError::DecompressionBomb {
                limit_bytes: limits.max_decompressed_bytes,
                bytes_read: total,
            });
        }
        out.extend_from_slice(&chunk[..n]);
    }

    Ok(out)
}

/// Single-stage decompression for one codec.
pub fn decompress_single(data: &[u8], coding: ContentCoding, limits: &DecoderLimits) -> Result<Vec<u8>, DecodeError> {
    match coding {
        ContentCoding::Identity => Ok(data.to_vec()),
        ContentCoding::Gzip => {
            let decoder = flate2::read::GzDecoder::new(data);
            read_with_bomb_limit(decoder, limits)
        }
        ContentCoding::Deflate => {
            // HTTP 'deflate' can either be zlib-wrapped (RFC 1950) or raw DEFLATE (RFC 1951).
            // Try zlib first; fallback to raw deflate if header fails.
            let zlib = flate2::read::ZlibDecoder::new(data);
            match read_with_bomb_limit(zlib, limits) {
                Ok(res) => Ok(res),
                Err(_) => {
                    let raw = flate2::read::DeflateDecoder::new(data);
                    read_with_bomb_limit(raw, limits)
                }
            }
        }
        ContentCoding::Brotli => {
            let decoder = brotli::Decompressor::new(data, 4096);
            read_with_bomb_limit(decoder, limits)
        }
        ContentCoding::Zstd => {
            let decoder = zstd::stream::read::Decoder::new(data)
                .map_err(|e| DecodeError::MalformedData(e.to_string()))?;
            read_with_bomb_limit(decoder, limits)
        }
    }
}

/// Parses stacked Content-Encoding header value (e.g. "gzip, br" -> [Gzip, Brotli]).
pub fn parse_content_encodings(header_val: &str) -> Result<Vec<ContentCoding>, DecodeError> {
    let mut codings = Vec::new();
    for part in header_val.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        match ContentCoding::parse(trimmed) {
            Some(c) => codings.push(c),
            None => return Err(DecodeError::UnsupportedEncoding(trimmed.to_string())),
        }
    }
    Ok(codings)
}

/// Main execution entrypoint for the HTTP Response Body Decode Pipeline.
///
/// 1. Runs chunked reassembly (stage 1) gated strictly by HTTP/1.1 and `Transfer-Encoding: chunked`.
///    HTTP/2 and HTTP/3 skip chunk parsing.
/// 2. Decompresses Content-Encoding (stage 2) in reverse order of application.
/// 3. Emits `ViewerPayload` ready for downstream viewer inspection.
pub fn process_response_body(
    raw_body: &[u8],
    http_version: HttpVersion,
    transfer_encoding: Option<&str>,
    content_encoding: Option<&str>,
    content_type: Option<&str>,
    limits: &DecoderLimits,
) -> Result<ViewerPayload, DecodeError> {
    let raw_size = raw_body.len();

    // Stage 1: Transfer-Encoding Reassembly
    let chunk_res = decode_transfer(http_version, transfer_encoding, raw_body)?;
    let reassembled_bytes = chunk_res.data;
    let trailers = chunk_res.trailers;

    // Stage 2: Content-Encoding Decompression
    let mut encodings_applied = Vec::new();
    let mut current_bytes = reassembled_bytes;

    if let Some(ce) = content_encoding {
        let codings = parse_content_encodings(ce)?;
        // Multiple encodings are applied in order during transit: A then B.
        // To decode, we apply in reverse order: un-B then un-A.
        for coding in codings.iter().rev() {
            if *coding == ContentCoding::Identity {
                continue;
            }
            current_bytes = decompress_single(&current_bytes, *coding, limits)?;
            encodings_applied.push(coding.as_str().to_string());
        }
    }

    let decoded_size = current_bytes.len();

    Ok(ViewerPayload {
        body: current_bytes,
        content_type: content_type.map(|s| s.to_string()),
        raw_size,
        decoded_size,
        encodings_applied,
        trailers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
    use flate2::Compression;
    use std::io::Write;

    fn encode_gzip(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn encode_zlib_deflate(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn encode_raw_deflate(data: &[u8]) -> Vec<u8> {
        let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn encode_brotli(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 6, 22);
            writer.write_all(data).unwrap();
        }
        out
    }

    fn encode_zstd(data: &[u8]) -> Vec<u8> {
        zstd::encode_all(std::io::Cursor::new(data), 3).unwrap()
    }

    #[test]
    fn test_identity_and_empty() {
        let limits = DecoderLimits::default();
        let text = b"hello uncompressed world";
        let payload = process_response_body(
            text,
            HttpVersion::Http11,
            None,
            Some("identity"),
            Some("text/plain"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, text);
        assert_eq!(payload.raw_size, text.len());
        assert_eq!(payload.decoded_size, text.len());
        assert_eq!(payload.content_type.as_deref(), Some("text/plain"));
        assert!(payload.trailers.is_empty());
    }

    #[test]
    fn test_codec_gzip() {
        let limits = DecoderLimits::default();
        let original = b"Ferrotap gzip test payload";
        let compressed = encode_gzip(original);

        let payload = process_response_body(
            &compressed,
            HttpVersion::Http11,
            None,
            Some("gzip"),
            Some("application/json"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, original);
        assert_eq!(payload.raw_size, compressed.len());
        assert_eq!(payload.decoded_size, original.len());
        assert_eq!(payload.encodings_applied, vec!["gzip"]);
    }

    #[test]
    fn test_codec_deflate_zlib_and_raw() {
        let limits = DecoderLimits::default();
        let original = b"Deflate test with zlib headers";
        let zlib_compressed = encode_zlib_deflate(original);
        let raw_compressed = encode_raw_deflate(original);

        let p1 = process_response_body(
            &zlib_compressed,
            HttpVersion::Http11,
            None,
            Some("deflate"),
            None,
            &limits,
        )
        .unwrap();
        assert_eq!(p1.body, original);

        let p2 = process_response_body(
            &raw_compressed,
            HttpVersion::Http11,
            None,
            Some("deflate"),
            None,
            &limits,
        )
        .unwrap();
        assert_eq!(p2.body, original);
    }

    #[test]
    fn test_codec_brotli() {
        let limits = DecoderLimits::default();
        let original = b"Brotli compressed text for Ferrotap";
        let compressed = encode_brotli(original);

        let payload = process_response_body(
            &compressed,
            HttpVersion::Http11,
            None,
            Some("br"),
            Some("text/html"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, original);
        assert_eq!(payload.raw_size, compressed.len());
        assert_eq!(payload.decoded_size, original.len());
        assert_eq!(payload.encodings_applied, vec!["br"]);
    }

    #[test]
    fn test_codec_zstd() {
        let limits = DecoderLimits::default();
        let original = b"Zstandard compressed high speed payload";
        let compressed = encode_zstd(original);

        let payload = process_response_body(
            &compressed,
            HttpVersion::Http11,
            None,
            Some("zstd"),
            Some("application/octet-stream"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, original);
        assert_eq!(payload.raw_size, compressed.len());
        assert_eq!(payload.decoded_size, original.len());
        assert_eq!(payload.encodings_applied, vec!["zstd"]);
    }

    #[test]
    fn test_stacked_encodings() {
        let limits = DecoderLimits::default();
        let original = b"Multiply encoded: first gzip then brotli";
        // Encoded as: input -> gzip -> brotli
        let gz = encode_gzip(original);
        let stacked = encode_brotli(&gz);

        // Header specifies transit order: gzip, br
        let payload = process_response_body(
            &stacked,
            HttpVersion::Http11,
            None,
            Some("gzip, br"),
            Some("text/plain"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, original);
        assert_eq!(payload.encodings_applied, vec!["br", "gzip"]);
    }

    #[test]
    fn test_decompression_bomb_limit() {
        let limits = DecoderLimits {
            max_decompressed_bytes: 50,
            initial_buffer_capacity: 16,
        };
        let large_data = vec![b'A'; 200];
        let compressed = encode_gzip(&large_data);

        let err = process_response_body(
            &compressed,
            HttpVersion::Http11,
            None,
            Some("gzip"),
            None,
            &limits,
        )
        .unwrap_err();

        match err {
            DecodeError::DecompressionBomb { limit_bytes, .. } => {
                assert_eq!(limit_bytes, 50);
            }
            other => panic!("Expected DecompressionBomb, got {:?}", other),
        }
    }

    #[test]
    fn test_chunked_plus_gzip_combined() {
        let limits = DecoderLimits::default();
        let original = b"Combined chunked transfer and gzip compression test";
        let compressed = encode_gzip(original);

        // Chunk into 2 parts and append trailers
        let half = compressed.len() / 2;
        let chunk1 = &compressed[..half];
        let chunk2 = &compressed[half..];

        let mut chunked_payload = Vec::new();
        chunked_payload.extend_from_slice(format!("{:X}\r\n", chunk1.len()).as_bytes());
        chunked_payload.extend_from_slice(chunk1);
        chunked_payload.extend_from_slice(b"\r\n");

        chunked_payload.extend_from_slice(format!("{:X}\r\n", chunk2.len()).as_bytes());
        chunked_payload.extend_from_slice(chunk2);
        chunked_payload.extend_from_slice(b"\r\n");

        // End chunk + Signature trailer
        chunked_payload.extend_from_slice(b"0\r\nSignature: test-sig-12345\r\n\r\n");

        let payload = process_response_body(
            &chunked_payload,
            HttpVersion::Http11,
            Some("chunked"),
            Some("gzip"),
            Some("application/json"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, original);
        assert_eq!(payload.raw_size, chunked_payload.len());
        assert_eq!(payload.decoded_size, original.len());
        assert_eq!(payload.trailers.len(), 1);
        assert_eq!(payload.trailers[0].name.to_ascii_lowercase(), "signature");
        assert_eq!(payload.trailers[0].value, "test-sig-12345");
    }

    #[test]
    fn test_http2_bypass() {
        let limits = DecoderLimits::default();
        let original = b"HTTP/2 raw DATA frame payload";
        let compressed = encode_gzip(original);

        // On HTTP/2, even if Transfer-Encoding was mistakenly provided, chunk parsing is bypassed
        let payload = process_response_body(
            &compressed,
            HttpVersion::H2,
            Some("chunked"),
            Some("gzip"),
            Some("text/plain"),
            &limits,
        )
        .unwrap();

        assert_eq!(payload.body, original);
        assert_eq!(payload.raw_size, compressed.len());
        assert_eq!(payload.decoded_size, original.len());
    }
}

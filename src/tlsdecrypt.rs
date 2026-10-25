//! G14 decrypt TLS in packet captures with a key log (the SSLKEYLOGFILE format browsers and
//! Ferrotap's own proxy write). TLS 1.3 (AES-128/256-GCM, ChaCha20-Poly1305) and TLS 1.2
//! (ECDHE / RSA with AES-GCM or ChaCha20-Poly1305). Works on the two reassembled byte streams
//! of one TCP connection.
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};

use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_128_GCM, AES_256_GCM, CHACHA20_POLY1305};
use aws_lc_rs::{hkdf, hmac};

/// Secrets by (label, client random).
#[derive(Debug, Default, Clone)]
pub struct KeyLog {
    pub secrets: HashMap<(String, Vec<u8>), Vec<u8>>,
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    // hex is ASCII; anything else (a corrupt or wrong file) would also break the 2-byte slicing
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn parse_keylog(text: &str) -> KeyLog {
    let mut k = KeyLog::default();
    for l in text.lines() {
        let mut it = l.split_whitespace();
        let (Some(label), Some(cr), Some(sec)) = (it.next(), it.next(), it.next()) else { continue };
        if label.starts_with('#') {
            continue;
        }
        if let (Some(cr), Some(sec)) = (unhex(cr), unhex(sec)) {
            k.secrets.insert((label.to_string(), cr), sec);
        }
    }
    k
}

impl KeyLog {
    fn get(&self, label: &str, cr: &[u8]) -> Option<&Vec<u8>> {
        self.secrets.get(&(label.to_string(), cr.to_vec()))
    }
}

/// TLS records of one direction: (content type, payload). A cut-off last record is dropped.
pub fn records(stream: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 5 <= stream.len() {
        let len = u16::from_be_bytes([stream[p + 3], stream[p + 4]]) as usize;
        if p + 5 + len > stream.len() {
            break;
        }
        out.push((stream[p], stream[p..p + 5 + len].to_vec()));
        p += 5 + len;
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hello {
    pub random: Vec<u8>,
    pub cipher: u16,
    pub tls13: bool,
}

/// Client random from the first ClientHello in the client's stream.
pub fn client_random(c2s: &[u8]) -> Option<Vec<u8>> {
    let (_, r) = records(c2s).into_iter().find(|(t, r)| *t == 22 && r.get(5) == Some(&1))?;
    r.get(11..43).map(|x| x.to_vec())
}

/// Server random, cipher suite and TLS 1.3 (supported_versions) from the ServerHello.
pub fn server_hello(s2c: &[u8]) -> Option<Hello> {
    let (_, r) = records(s2c).into_iter().find(|(t, r)| *t == 22 && r.get(5) == Some(&2))?;
    let random = r.get(11..43)?.to_vec();
    let mut p = 43;
    let sid = *r.get(p)? as usize;
    p += 1 + sid;
    let cipher = u16::from_be_bytes([*r.get(p)?, *r.get(p + 1)?]);
    p += 3; // cipher + compression
    let mut tls13 = false;
    if let Some(l) = r.get(p..p + 2) {
        let end = (p + 2 + u16::from_be_bytes([l[0], l[1]]) as usize).min(r.len());
        p += 2;
        while p + 4 <= end {
            let t = u16::from_be_bytes([r[p], r[p + 1]]);
            let n = u16::from_be_bytes([r[p + 2], r[p + 3]]) as usize;
            if t == 0x002b && r.get(p + 4..p + 6) == Some(&[3, 4]) {
                tls13 = true;
            }
            p += 4 + n;
        }
    }
    Some(Hello { random, cipher, tls13 })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Aead {
    Aes128,
    Aes256,
    Chacha,
}

impl Aead {
    fn key_len(self) -> usize {
        match self {
            Aead::Aes128 => 16,
            _ => 32,
        }
    }
    fn alg(self) -> &'static aws_lc_rs::aead::Algorithm {
        match self {
            Aead::Aes128 => &AES_128_GCM,
            Aead::Aes256 => &AES_256_GCM,
            Aead::Chacha => &CHACHA20_POLY1305,
        }
    }
}

/// (AEAD, SHA-384 hash?, name) for the supported suites.
fn suite(c: u16) -> Option<(Aead, bool, &'static str)> {
    Some(match c {
        0x1301 => (Aead::Aes128, false, "TLS_AES_128_GCM_SHA256"),
        0x1302 => (Aead::Aes256, true, "TLS_AES_256_GCM_SHA384"),
        0x1303 => (Aead::Chacha, false, "TLS_CHACHA20_POLY1305_SHA256"),
        0xc02f | 0xc02b | 0x009c | 0x009e => (Aead::Aes128, false, "AES_128_GCM_SHA256"),
        0xc030 | 0xc02c | 0x009d | 0x009f => (Aead::Aes256, true, "AES_256_GCM_SHA384"),
        0xcca8..=0xccaa => (Aead::Chacha, false, "CHACHA20_POLY1305_SHA256"),
        _ => return None,
    })
}

struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand_label(secret: &[u8], sha384: bool, label: &str, n: usize) -> Vec<u8> {
    let alg = if sha384 { hkdf::HKDF_SHA384 } else { hkdf::HKDF_SHA256 };
    let full = format!("tls13 {label}");
    let mut info = Vec::new();
    info.extend((n as u16).to_be_bytes());
    info.push(full.len() as u8);
    info.extend(full.as_bytes());
    info.push(0);
    let prk = hkdf::Prk::new_less_safe(alg, secret);
    let mut out = vec![0u8; n];
    let parts = [info.as_slice()];
    prk.expand(&parts, Len(n)).and_then(|o| o.fill(&mut out)).expect("hkdf");
    out
}

fn prf(secret: &[u8], label: &str, seed: &[u8], n: usize, sha384: bool) -> Vec<u8> {
    let key = hmac::Key::new(if sha384 { hmac::HMAC_SHA384 } else { hmac::HMAC_SHA256 }, secret);
    let mut s = label.as_bytes().to_vec();
    s.extend(seed);
    let mut a = hmac::sign(&key, &s).as_ref().to_vec();
    let mut out = Vec::new();
    while out.len() < n {
        let mut m = a.clone();
        m.extend(&s);
        out.extend(hmac::sign(&key, &m).as_ref());
        a = hmac::sign(&key, &a).as_ref().to_vec();
    }
    out.truncate(n);
    out
}

fn open(aead: Aead, key: &[u8], nonce: [u8; 12], aad: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let k = LessSafeKey::new(UnboundKey::new(aead.alg(), key).ok()?);
    let mut buf = data.to_vec();
    let plain = k.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf).ok()?;
    Some(plain.to_vec())
}

fn xor_nonce(iv: &[u8], seq: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n.copy_from_slice(&iv[..12]);
    for (i, b) in seq.to_be_bytes().iter().enumerate() {
        n[4 + i] ^= b;
    }
    n
}

/// One direction of TLS 1.3: try the handshake keys first, then the application keys.
fn tls13_direction(stream: &[u8], aead: Aead, sha384: bool, hs: Option<&Vec<u8>>, app: Option<&Vec<u8>>) -> (Vec<u8>, usize) {
    let derive = |s: &Vec<u8>| (expand_label(s, sha384, "key", aead.key_len()), expand_label(s, sha384, "iv", 12));
    let hs = hs.map(derive);
    let app = app.map(derive);
    let (mut seq_h, mut seq_a, mut in_app) = (0u64, 0u64, false);
    let (mut out, mut failed) = (Vec::new(), 0);
    for (t, rec) in records(stream) {
        if t != 23 {
            continue;
        }
        let (aad, body) = rec.split_at(5);
        let mut plain = None;
        if !in_app {
            if let Some((k, iv)) = &hs {
                plain = open(aead, k, xor_nonce(iv, seq_h), aad, body);
                if plain.is_some() {
                    seq_h += 1;
                }
            }
        }
        if plain.is_none() {
            if let Some((k, iv)) = &app {
                plain = open(aead, k, xor_nonce(iv, seq_a), aad, body);
                if plain.is_some() {
                    seq_a += 1;
                    in_app = true;
                }
            }
        }
        match plain {
            Some(mut p) => {
                while p.last() == Some(&0) {
                    p.pop();
                }
                if p.pop() == Some(23) {
                    out.extend(p);
                }
            }
            None => failed += 1,
        }
    }
    (out, failed)
}

/// One direction of TLS 1.2 (records after ChangeCipherSpec).
fn tls12_direction(stream: &[u8], aead: Aead, key: &[u8], iv: &[u8]) -> (Vec<u8>, usize) {
    let (mut seq, mut on, mut out, mut failed) = (0u64, false, Vec::new(), 0);
    for (t, rec) in records(stream) {
        if t == 20 {
            on = true;
            continue;
        }
        if !on || !(t == 22 || t == 23 || t == 21) {
            continue;
        }
        let body = &rec[5..];
        let (nonce, ct) = if aead == Aead::Chacha {
            (xor_nonce(iv, seq), body)
        } else {
            if body.len() < 8 + 16 {
                failed += 1;
                continue;
            }
            let mut n = [0u8; 12];
            n[..4].copy_from_slice(&iv[..4]);
            n[4..].copy_from_slice(&body[..8]);
            (n, &body[8..])
        };
        let plain_len = ct.len().saturating_sub(16) as u16;
        let mut aad = seq.to_be_bytes().to_vec();
        aad.push(t);
        aad.extend(&rec[1..3]);
        aad.extend(plain_len.to_be_bytes());
        match open(aead, key, nonce, &aad, ct) {
            Some(p) => {
                if t == 23 {
                    out.extend(p);
                }
            }
            None => failed += 1,
        }
        seq += 1;
    }
    (out, failed)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decrypted {
    pub version: &'static str,
    pub cipher: &'static str,
    /// Application data client -> server and server -> client.
    pub client: Vec<u8>,
    pub server: Vec<u8>,
    /// Records that could not be decrypted.
    pub failed: usize,
}

/// Decrypt one TLS connection given both directions' bytes and the key log.
pub fn decrypt(c2s: &[u8], s2c: &[u8], keys: &KeyLog) -> Result<Decrypted, String> {
    let cr = client_random(c2s).ok_or("no ClientHello in the client's data")?;
    let sh = server_hello(s2c).ok_or("no ServerHello in the server's data")?;
    let (aead, sha384, name) = suite(sh.cipher).ok_or_else(|| format!("cipher suite 0x{:04x} is not supported", sh.cipher))?;
    if sh.tls13 {
        let get = |l: &str| keys.get(l, &cr);
        if get("CLIENT_TRAFFIC_SECRET_0").is_none() && get("SERVER_TRAFFIC_SECRET_0").is_none() {
            return Err("the key log has no TLS 1.3 secrets for this connection".into());
        }
        let (client, f1) = tls13_direction(c2s, aead, sha384, get("CLIENT_HANDSHAKE_TRAFFIC_SECRET"), get("CLIENT_TRAFFIC_SECRET_0"));
        let (server, f2) = tls13_direction(s2c, aead, sha384, get("SERVER_HANDSHAKE_TRAFFIC_SECRET"), get("SERVER_TRAFFIC_SECRET_0"));
        Ok(Decrypted { version: "TLS 1.3", cipher: name, client, server, failed: f1 + f2 })
    } else {
        let ms = keys.get("CLIENT_RANDOM", &cr).ok_or("the key log has no CLIENT_RANDOM line for this connection")?;
        let mut seed = sh.random.clone();
        seed.extend(&cr);
        let kl = aead.key_len();
        let ivl = if aead == Aead::Chacha { 12 } else { 4 };
        let kb = prf(ms, "key expansion", &seed, 2 * kl + 2 * ivl, sha384);
        let (ck, rest) = kb.split_at(kl);
        let (sk, rest) = rest.split_at(kl);
        let (civ, siv) = rest.split_at(ivl);
        let (client, f1) = tls12_direction(c2s, aead, ck, civ);
        let (server, f2) = tls12_direction(s2c, aead, sk, siv);
        Ok(Decrypted { version: "TLS 1.2", cipher: name, client, server, failed: f1 + f2 })
    }
}

// ------------------------------------------------------------------------------------------ writing key logs

fn keylog_file() -> &'static Mutex<Option<std::path::PathBuf>> {
    static F: OnceLock<Mutex<Option<std::path::PathBuf>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(None))
}

/// Where Ferrotap appends key log lines (None = off).
pub fn set_keylog_path(p: Option<std::path::PathBuf>) {
    if let Ok(mut f) = keylog_file().lock() {
        *f = p;
    }
}

/// The key log file: a path set by `set_keylog_path` (tests), else Connections > TLS key log.
pub fn keylog_path() -> Option<std::path::PathBuf> {
    keylog_file().lock().ok().and_then(|f| f.clone()).or_else(|| {
        let s = crate::conn::current().keylog;
        (!s.trim().is_empty()).then(|| std::path::PathBuf::from(s.trim()))
    })
}

/// The proxy's upstream TLS secrets, appended to the key log file when one is set.
#[derive(Debug)]
pub struct ProxyKeyLog;

impl hudsucker::rustls::KeyLog for ProxyKeyLog {
    fn log(&self, label: &str, client_random: &[u8], secret: &[u8]) {
        let Some(p) = keylog_path() else { return };
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            let _ = writeln!(f, "{label} {} {}", hex(client_random), hex(secret));
        }
    }

    fn will_log(&self, _label: &str) -> bool {
        keylog_path().is_some()
    }
}

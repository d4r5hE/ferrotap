//! G14 TLS decryption with a key log, checked against real rustls connections (TLS 1.3 with
//! AES-GCM and ChaCha20, TLS 1.2 with AES-GCM). Bytes are moved in memory and recorded per
//! direction, like the two halves of a captured TCP connection.
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use ferrotap::tlsdecrypt::*;
use hudsucker::rustls::{
    self,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
};

#[derive(Debug, Default)]
struct Log(Mutex<String>);

impl rustls::KeyLog for Log {
    fn log(&self, label: &str, cr: &[u8], secret: &[u8]) {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        self.0.lock().unwrap().push_str(&format!("{label} {} {}\n", hex(cr), hex(secret)));
    }
}

/// Run one exchange; returns (client->server bytes, server->client bytes, key log text).
fn exchange(versions: &[&'static rustls::SupportedProtocolVersion], suites: Option<Vec<rustls::SupportedCipherSuite>>) -> (Vec<u8>, Vec<u8>, String) {
    let ck = rcgen::generate_simple_self_signed(vec!["secure.test".to_string()]).unwrap();
    let der = ck.cert.der().clone();
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
    let mut provider = aws_lc_rs::default_provider();
    if let Some(s) = suites {
        provider.cipher_suites = s;
    }
    let provider = Arc::new(provider);
    let scfg = rustls::ServerConfig::builder_with_provider(provider.clone()).with_protocol_versions(versions).unwrap().with_no_client_auth().with_single_cert(vec![der.clone()], key).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(der)).unwrap();
    let log = Arc::new(Log::default());
    let mut ccfg = rustls::ClientConfig::builder_with_provider(provider).with_protocol_versions(versions).unwrap().with_root_certificates(roots).with_no_client_auth();
    ccfg.key_log = log.clone();
    let mut client = rustls::ClientConnection::new(Arc::new(ccfg), ServerName::try_from("secure.test").unwrap()).unwrap();
    let mut server = rustls::ServerConnection::new(Arc::new(scfg)).unwrap();
    let (mut c2s, mut s2c) = (Vec::new(), Vec::new());
    let mut pump = |client: &mut rustls::ClientConnection, server: &mut rustls::ServerConnection| {
        for _ in 0..20 {
            let mut buf = Vec::new();
            while client.wants_write() {
                client.write_tls(&mut buf).unwrap();
            }
            c2s.extend(&buf);
            let mut rd = &buf[..];
            while !rd.is_empty() {
                server.read_tls(&mut rd).unwrap();
            }
            server.process_new_packets().unwrap();
            let mut buf = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut buf).unwrap();
            }
            s2c.extend(&buf);
            let mut rd = &buf[..];
            while !rd.is_empty() {
                client.read_tls(&mut rd).unwrap();
            }
            client.process_new_packets().unwrap();
        }
    };
    pump(&mut client, &mut server);
    client.writer().write_all(b"GET /secret HTTP/1.1\r\nHost: secure.test\r\n\r\n").unwrap();
    pump(&mut client, &mut server);
    let mut got = vec![0u8; 256];
    let n = server.reader().read(&mut got).unwrap();
    assert!(got[..n].starts_with(b"GET /secret"));
    server.writer().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nhidden").unwrap();
    pump(&mut client, &mut server);
    let log = log.0.lock().unwrap().clone();
    (c2s, s2c, log)
}

#[test]
fn tls13_aes_gcm() {
    let (c2s, s2c, log) = exchange(&[&rustls::version::TLS13], None);
    assert!(log.contains("CLIENT_TRAFFIC_SECRET_0"));
    let keys = parse_keylog(&log);
    let d = decrypt(&c2s, &s2c, &keys).unwrap();
    assert_eq!(d.version, "TLS 1.3");
    assert!(d.cipher.contains("GCM"), "{}", d.cipher);
    assert_eq!(d.client, b"GET /secret HTTP/1.1\r\nHost: secure.test\r\n\r\n");
    assert!(String::from_utf8_lossy(&d.server).contains("hidden"), "{:?}", String::from_utf8_lossy(&d.server));
    // without keys: a clear error
    assert!(decrypt(&c2s, &s2c, &KeyLog::default()).unwrap_err().contains("no TLS 1.3 secrets"));
}

#[test]
fn tls13_chacha() {
    let suites = vec![aws_lc_rs::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256];
    let (c2s, s2c, log) = exchange(&[&rustls::version::TLS13], Some(suites));
    let d = decrypt(&c2s, &s2c, &parse_keylog(&log)).unwrap();
    assert_eq!(d.cipher, "TLS_CHACHA20_POLY1305_SHA256");
    assert!(d.client.starts_with(b"GET /secret"));
}

#[test]
fn tls12_aes_gcm() {
    let suites = vec![aws_lc_rs::cipher_suite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384];
    let (c2s, s2c, log) = exchange(&[&rustls::version::TLS12], Some(suites));
    assert!(log.contains("CLIENT_RANDOM"));
    let d = decrypt(&c2s, &s2c, &parse_keylog(&log)).unwrap();
    assert_eq!(d.version, "TLS 1.2");
    assert_eq!(d.cipher, "AES_256_GCM_SHA384");
    assert!(d.client.starts_with(b"GET /secret"), "{:?}", String::from_utf8_lossy(&d.client));
    assert!(String::from_utf8_lossy(&d.server).ends_with("hidden"));
    assert_eq!(d.failed, 0);
}

#[test]
fn keylog_parsing_and_writer() {
    let k = parse_keylog("# comment\nCLIENT_RANDOM 0a0b 0c0d\nbad line\nX zz 00\n");
    assert_eq!(k.secrets.len(), 1);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("keys.log");
    set_keylog_path(Some(p.clone()));
    use hudsucker::rustls::KeyLog as _;
    assert!(ProxyKeyLog.will_log("x"));
    ProxyKeyLog.log("CLIENT_RANDOM", &[1, 2], &[3, 4]);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "CLIENT_RANDOM 0102 0304\n");
    set_keylog_path(None);
    assert!(!ProxyKeyLog.will_log("x"));
}

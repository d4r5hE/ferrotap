//! Deterministic fuzzing: random and mutated inputs must never panic (or hang) any parser that
//! reads captured data or user files.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max + 1);
        (0..n).map(|_| self.next() as u8).collect()
    }
    /// A seed with a few bytes flipped, cut, or repeated.
    fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut v = seed.to_vec();
        for _ in 0..1 + self.below(6) {
            if v.is_empty() {
                break;
            }
            match self.below(4) {
                0 => {
                    let i = self.below(v.len());
                    v[i] = self.next() as u8;
                }
                1 => v.truncate(self.below(v.len())),
                2 => {
                    let i = self.below(v.len());
                    let b = self.next() as u8;
                    v.insert(i, b);
                }
                _ => {
                    let i = self.below(v.len());
                    let j = (i + self.below(16)).min(v.len());
                    let chunk = v[i..j].to_vec();
                    v.splice(i..i, chunk);
                }
            }
        }
        v
    }
}

fn check(name: &str, input: &[u8], f: &dyn Fn(&[u8])) {
    let t = Instant::now();
    let r = catch_unwind(AssertUnwindSafe(|| f(input)));
    let took = t.elapsed();
    let show = |b: &[u8]| b.iter().take(80).map(|x| format!("{x:02x}")).collect::<String>();
    assert!(r.is_ok(), "{name} panicked on input ({} bytes) {}", input.len(), show(input));
    assert!(took < Duration::from_secs(2), "{name} took {took:?} on input ({} bytes) {}", input.len(), show(input));
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn parsers_survive_random_and_mutated_input() {
    use ferrotap::*;
    // seeds: valid inputs to mutate
    let frame = pcap::Frame { ts: 1.0, linktype: 1, orig_len: 60, data: vec![0u8; 12].into_iter().chain([0x08, 0x00, 0x45, 0, 0, 40, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2, 0, 80, 0, 81, 0, 0, 0, 1, 0, 0, 0, 0, 0x50, 0x18, 0xff, 0xff, 0, 0, 0, 0]).collect() };
    let pcap_seed = pcap::write_pcap(&[&frame, &frame]);
    let mp_seed: Vec<u8> = vec![0x93, 0x01, 0xa3, b'a', b'b', b'c', 0x82, 0xa1, b'k', 0xc3, 0xa1, b'n', 0xcd, 0x01, 0x00];
    let pb_seed: Vec<u8> = vec![0x08, 0x96, 0x01, 0x12, 0x03, b'a', b'b', b'c', 0x1a, 0x02, 0x08, 0x01];
    let grpc_seed: Vec<u8> = [&[0u8, 0, 0, 0, 12][..], &pb_seed].concat();
    let har_seed = br#"{"log":{"version":"1.2","entries":[{"startedDateTime":"2026-01-01T00:00:00Z","time":1,"request":{"method":"GET","url":"https://a.test/","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":0},"response":{"status":200,"statusText":"OK","httpVersion":"HTTP/1.1","headers":[],"cookies":[],"content":{"size":2,"mimeType":"text/plain","text":"aGk=","encoding":"base64"},"redirectURL":"","headersSize":-1,"bodySize":2},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}}"#.to_vec();
    let farx_seed = br#"<?xml version="1.0"?><AutoResponder LastSave="x" FiddlerVersion="5"><State Enabled="true"><ResponseRule Match="regex:.*a\.test" Action="*404" Enabled="true" /><ResponseRule Match="EXACT:https://b.test/" Action="C:\x.txt" Enabled="false" /></State></AutoResponder>"#.to_vec();
    let proto_seed = b"syntax = \"proto3\"; package p; message M { int32 a = 1; repeated string b = 2; map<string, M> m = 3; oneof o { bool c = 4; } enum E { X = 0; } }".to_vec();
    let sse_seed = b"event: x\ndata: {\"a\":1}\nid: 3\nretry: 10\n\ndata: second\n\n".to_vec();
    let markup_seed = b"<html><head><title>T</title><script>if(a<b){}</script></head><body><p class='x'>Hi <b>there</b><br/><img src=x></p><!-- c --></body></html>".to_vec();

    type F = Box<dyn Fn(&[u8])>;
    let targets: Vec<(&str, Vec<u8>, F)> = vec![
        ("pcap::parse", pcap_seed.clone(), Box::new(|b| { if let Ok(fr) = pcap::parse(b) { let pk = pcap::decode_all(&fr); let refs: Vec<_> = pk.iter().collect(); let _ = pcap::conversations(&refs); if let Some(p) = pk.first() { let _ = pcap::follow_tcp(&fr, &pk, p); } } })),
        ("pcap::decode frame", frame.data.clone(), Box::new(|b| { let f = pcap::Frame { ts: 0.0, linktype: 1, orig_len: b.len() as u32, data: b.to_vec() }; let _ = pcap::decode(1, &f, 0.0); let _ = pcap::parse_dns(b); let _ = pcap::tls_sni(b); })),
        ("pfilter::parse", b"tcp.port == 443 and not (dns or ip.src == 10.0.0.1) && frame.len > 10".to_vec(), Box::new(|b| { let _ = pfilter::parse(&text(b)); let _ = pfilter::complete(&text(b)); })),
        ("bpdsl::parse", b"method == \"POST\" && url contains \"api\" || status >= 400".to_vec(), Box::new(|b| { let _ = bpdsl::parse(&text(b)); })),
        ("msgpack", mp_seed.clone(), Box::new(|b| { let _ = streams_decode::mp_decode_all(b); let _ = streams_decode::msgpack_to_text(b); let _ = streams_decode::decode_signalr_msgpack(b); })),
        ("protobuf", pb_seed.clone(), Box::new(|b| { if let Some(f) = streams_decode::pb_decode(b) { let _ = streams_decode::pb_to_text(&f, 0); } })),
        ("grpc frames", grpc_seed.clone(), Box::new(|b| { let _ = streams_decode::grpc_frames(b); })),
        ("sse / socket.io / signalr", sse_seed.clone(), Box::new(|b| { let t = text(b); let _ = streams_decode::parse_sse(&t); let _ = streams_decode::decode_socketio(&t); let _ = streams_decode::decode_signalr_json(&t); })),
        ("har", har_seed.clone(), Box::new(|b| { let _ = har::parse_har(&text(b)); })),
        ("ftap", har_seed.clone(), Box::new(|b| { let _ = data_io::parse_ftap(&text(b)); })),
        ("saz", Vec::new(), Box::new(|b| { let _ = saz::parse_saz(b); })),
        ("farx", farx_seed.clone(), Box::new(|b| { let _ = farx::parse_farx(&text(b)); })),
        ("proto schema", proto_seed.clone(), Box::new(|b| { if let Ok(s) = protoschema::parse(&text(b)) { let _ = protoschema::decode_message(&s, "p.M", b, 0); } })),
        ("tls records", Vec::new(), Box::new(|b| { let _ = tlsdecrypt::records(b); let _ = tlsdecrypt::client_random(b); let _ = tlsdecrypt::server_hello(b); let _ = tlsdecrypt::parse_keylog(&text(b)); let _ = tlsdecrypt::decrypt(b, b, &tlsdecrypt::parse_keylog("")); })),
        ("markup / js / json pretty", markup_seed.clone(), Box::new(|b| { let t = text(b); let _ = inspect::pretty_markup(&t, true); let _ = inspect::pretty_markup(&t, false); let _ = inspect::pretty_js(&t); let _ = htmlpreview::render(&t); })),
        ("cookies / query / forms", b"a=1; b=%ZZ; c=\"q\"; Expires=Wed, 21 Oct 2015 07:28:00 GMT; SameSite=Lax".to_vec(), Box::new(|b| { let t = text(b); let _ = inspect::parse_cookie_header(&t); let _ = inspect::parse_set_cookie(&t); let _ = inspect::split_set_cookie_header(&t); let _ = inspect::parse_query(&format!("https://x.test/?{t}")); let _ = inspect::parse_urlencoded(&t); let _ = inspect::url_decode(&t, true); })),
        ("content decoding", vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 0x4b, 0x4c, 0x4a, 6, 0, 0xc2, 0x41, 0x24, 0x35, 3, 0, 0, 0], Box::new(|b| { for ce in ["gzip", "deflate", "br", "zstd", "gzip, br", "x-unknown"] { let _ = inspect::decode_bytes(b, ce); } })),
    ];
    // FUZZ_ITERS / FUZZ_SEED run it longer or differently (defaults keep the suite quick)
    let iters: usize = std::env::var("FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let seed: u64 = std::env::var("FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x9e37_79b9_7f4a_7c15);
    let mut rng = Rng(seed.max(1));
    for (name, seed, f) in &targets {
        check(name, seed, f.as_ref());
        check(name, &[], f.as_ref());
        for i in 0..iters {
            let input = if seed.is_empty() || i % 3 == 0 { rng.bytes(600) } else { rng.mutate(seed) };
            check(name, &input, f.as_ref());
        }
    }
}

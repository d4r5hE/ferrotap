//! G19 packet files: pcap / pcapng reading and writing, decoding, statistics, follow stream, filters.
use ferrotap::pcap::*;
use ferrotap::pfilter;

fn ipv4_tcp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, seq: u32, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; 12];
    f.extend([0x08, 0x00]);
    let total = 20 + 20 + payload.len();
    f.extend([0x45, 0, (total >> 8) as u8, total as u8, 0, 0, 0, 0, 64, 6, 0, 0]);
    f.extend(src);
    f.extend(dst);
    f.extend(sport.to_be_bytes());
    f.extend(dport.to_be_bytes());
    f.extend(seq.to_be_bytes());
    f.extend(0u32.to_be_bytes());
    f.extend([0x50, flags, 0xff, 0xff, 0, 0, 0, 0]);
    f.extend(payload);
    f
}

fn ipv4_udp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; 12];
    f.extend([0x08, 0x00]);
    let total = 20 + 8 + payload.len();
    f.extend([0x45, 0, (total >> 8) as u8, total as u8, 0, 0, 0, 0, 64, 17, 0, 0]);
    f.extend(src);
    f.extend(dst);
    f.extend(sport.to_be_bytes());
    f.extend(dport.to_be_bytes());
    f.extend(((8 + payload.len()) as u16).to_be_bytes());
    f.extend([0, 0]);
    f.extend(payload);
    f
}

fn dns_query(name: &str, response: bool, answer: Option<[u8; 4]>) -> Vec<u8> {
    let mut m = vec![0x12, 0x34, if response { 0x81 } else { 0x01 }, 0x00, 0, 1, 0, answer.is_some() as u8, 0, 0, 0, 0];
    for part in name.split('.') {
        m.push(part.len() as u8);
        m.extend(part.as_bytes());
    }
    m.extend([0, 0, 1, 0, 1]);
    if let Some(ip) = answer {
        m.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        m.extend(ip);
    }
    m
}

fn client_hello(sni: &str) -> Vec<u8> {
    let mut ext = vec![0, 0];
    let list_len = 3 + sni.len();
    ext.extend(((list_len + 2) as u16).to_be_bytes());
    ext.extend((list_len as u16).to_be_bytes());
    ext.push(0);
    ext.extend((sni.len() as u16).to_be_bytes());
    ext.extend(sni.as_bytes());
    let mut body = vec![3, 3];
    body.extend([0u8; 32]);
    body.push(0); // session id
    body.extend([0, 2, 0x13, 0x01]); // cipher suites
    body.extend([1, 0]); // compression
    body.extend((ext.len() as u16).to_be_bytes());
    body.extend(ext);
    let mut hs = vec![1, 0, (body.len() >> 8) as u8, body.len() as u8];
    hs.extend(body);
    let mut rec = vec![0x16, 3, 1, (hs.len() >> 8) as u8, hs.len() as u8];
    rec.extend(hs);
    rec
}

fn frames() -> Vec<Frame> {
    let c = [192, 168, 1, 10];
    let s = [93, 184, 216, 34];
    let dns = [8, 8, 8, 8];
    let mk = |ts: f64, data: Vec<u8>| Frame { ts, linktype: 1, orig_len: data.len() as u32, data };
    vec![
        mk(1000.0, ipv4_udp(c, dns, 5555, 53, &dns_query("example.com", false, None))),
        mk(1000.1, ipv4_udp(dns, c, 53, 5555, &dns_query("example.com", true, Some(s)))),
        mk(1000.2, ipv4_tcp(c, s, 50000, 80, 1, 0x02, b"")),
        mk(1000.3, ipv4_tcp(c, s, 50000, 80, 1, 0x18, b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n")),
        mk(1000.3, ipv4_tcp(c, s, 50000, 80, 1, 0x18, b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n")), // retransmission
        mk(1001.4, ipv4_tcp(s, c, 80, 50000, 900, 0x18, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")),
        mk(1002.5, ipv4_tcp(c, s, 50001, 443, 7, 0x18, &client_hello("api.example.com"))),
    ]
}

#[test]
fn decode_and_statistics() {
    let fr = frames();
    let pk = decode_all(&fr);
    assert_eq!(pk[0].protocol, "DNS");
    assert!(pk[1].info.contains("response A example.com -> 93.184.216.34"), "{}", pk[1].info);
    assert_eq!(pk[2].protocol, "TCP");
    assert!(pk[2].info.contains("[SYN]"));
    assert_eq!(pk[3].protocol, "HTTP");
    assert_eq!(pk[3].info, "GET /index.html HTTP/1.1");
    assert_eq!(pk[6].protocol, "TLS");
    assert_eq!(pk[6].sni.as_deref(), Some("api.example.com"));
    assert!((pk[5].time - 1.4).abs() < 1e-6);

    let refs: Vec<&Packet> = pk.iter().collect();
    let conv = conversations(&refs);
    assert_eq!(conv.len(), 3);
    assert!(conv.iter().any(|c| c.transport == "TCP" && c.packets == 4 && c.b == "93.184.216.34:80" || c.a == "93.184.216.34:80"));
    let ep = endpoints(&refs);
    assert_eq!(ep[0].0, "192.168.1.10", "busiest endpoint first");
    let h = hierarchy(&refs);
    assert!(h.iter().any(|(p, n, _)| p == "eth/ipv4/udp/dns" && *n == 2));
    assert!(h.iter().any(|(p, n, _)| p == "eth/ipv4/tcp" && *n == 5));
    let io = io_graph(&refs, 1.0);
    assert_eq!(io.iter().map(|x| x.0).collect::<Vec<_>>(), vec![5, 1, 1]);
    let dl = dns_log(&refs);
    assert_eq!(dl.len(), 2);
    assert_eq!((dl[1].1, dl[1].3.as_str(), dl[1].4.as_str()), (true, "example.com", "93.184.216.34"));

    let (up, down, chunks) = follow_tcp(&fr, &pk, &pk[3]);
    assert_eq!(chunks.len(), 2, "client then server, retransmission dropped");
    assert!(chunks[0].0 && !chunks[1].0);
    assert!(String::from_utf8_lossy(&chunks[1].1).ends_with("hi"));
    assert_eq!(up, 47);
    assert!(down > 30);

    let mut sess = ferrotap::model::SessionRecord { id: 7, url: "http://example.com/index.html".into(), remote_ip: Some("93.184.216.34".into()), start_time: chrono::DateTime::from_timestamp(1000, 300_000_000).unwrap().to_rfc3339(), ..Default::default() };
    let other = ferrotap::model::SessionRecord { id: 8, remote_ip: Some("1.1.1.1".into()), ..Default::default() };
    let http_conv = conv.iter().find(|c| c.a.ends_with(":80") || c.b.ends_with(":80")).unwrap();
    assert_eq!(related_sessions(http_conv, &[sess.clone(), other], 1000.0), vec![7]);
    sess.start_time = chrono::DateTime::from_timestamp(5000, 0).unwrap().to_rfc3339();
    assert!(related_sessions(http_conv, &[sess], 1000.0).is_empty(), "too far apart in time");
}

#[test]
fn pcap_round_trip_and_pcapng() {
    let fr = frames();
    let refs: Vec<&Frame> = fr.iter().collect();
    let bytes = write_pcap(&refs);
    let back = parse(&bytes).unwrap();
    assert_eq!(back.len(), fr.len());
    assert_eq!(back[3].data, fr[3].data);
    assert!((back[5].ts - 1001.4).abs() < 1e-5);

    // pcapng: section header, interface (microseconds), two enhanced packets
    let mut ng = Vec::new();
    let block = |t: u32, body: &[u8]| {
        let mut b = Vec::new();
        let len = (12 + body.len()) as u32;
        b.extend(t.to_le_bytes());
        b.extend(len.to_le_bytes());
        b.extend(body);
        b.extend(len.to_le_bytes());
        b
    };
    let mut shb = 0x1a2b3c4du32.to_le_bytes().to_vec();
    shb.extend([1, 0, 0, 0]);
    shb.extend((-1i64).to_le_bytes());
    ng.extend(block(0x0a0d0d0a, &shb));
    let mut idb = vec![1, 0, 0, 0];
    idb.extend(0u32.to_le_bytes());
    ng.extend(block(1, &idb));
    for f in &fr[..2] {
        let us = (f.ts * 1e6) as u64;
        let mut epb = 0u32.to_le_bytes().to_vec();
        epb.extend(((us >> 32) as u32).to_le_bytes());
        epb.extend((us as u32).to_le_bytes());
        epb.extend((f.data.len() as u32).to_le_bytes());
        epb.extend((f.data.len() as u32).to_le_bytes());
        epb.extend(&f.data);
        while epb.len() % 4 != 0 {
            epb.push(0);
        }
        ng.extend(block(6, &epb));
    }
    let p = parse(&ng).unwrap();
    assert_eq!(p.len(), 2);
    assert!((p[1].ts - 1000.1).abs() < 1e-5);
    assert_eq!(decode_all(&p)[1].protocol, "DNS");
    assert!(parse(b"nope").is_err());
}

#[test]
fn display_filters() {
    let pk = decode_all(&frames());
    let run = |f: &str| -> Vec<usize> {
        let e = pfilter::parse(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        pk.iter().filter(|p| e.as_ref().is_none_or(|e| pfilter::eval(e, p))).map(|p| p.no).collect()
    };
    assert_eq!(run(""), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(run("dns"), vec![1, 2]);
    assert_eq!(run("tcp.port == 443"), vec![7]);
    assert_eq!(run("tls.sni contains \"API\""), vec![7]);
    assert_eq!(run("http && ip.src == 93.184.216.34"), vec![6]);
    assert_eq!(run("tcp.flags.syn == 1"), vec![3]);
    assert_eq!(run("not tcp and not dns.flags.response == 1"), vec![1]);
    assert_eq!(run("(udp || http) and frame.len > 60"), vec![1, 2, 4, 5, 6]);
    assert_eq!(run("dns.qry.name matches \"^ex.*com$\""), vec![1, 2]);
    assert_eq!(run("ip.addr != 8.8.8.8"), vec![3, 4, 5, 6, 7]);
    // CIDR subnet matching
    assert!(run("ip.addr == 93.184.216.0/24").contains(&6));
    assert!(!run("ip.addr == 10.0.0.0/8").contains(&6));
    assert_eq!(run("ip.addr == 8.8.8.8/32"), run("ip.addr == 8.8.8.8"));
    assert_eq!(run("ip.addr == 0.0.0.0/0"), vec![1, 2, 3, 4, 5, 6, 7]); // /0 matches every IPv4 packet
    assert_eq!(run("ip.addr != 8.8.8.0/24"), vec![3, 4, 5, 6, 7]);
    assert!(run("ip.addr == 8.8.8.8/999").is_empty()); // invalid prefix: no match, no panic
    assert_eq!(run("frame.number ge 6"), vec![6, 7]);
    // Added fields: TCP sequence, DNS query type and A-record answers.
    assert_eq!(run("dns.qry.type == 1"), vec![1, 2]); // both are A queries
    assert_eq!(run("dns.a == 93.184.216.34"), vec![2]); // only the response carries an answer
    assert_eq!(run("tcp.seq == 900"), vec![6]);
    assert_eq!(run("tcp.seq in {7 900}"), vec![6, 7]);
    // HTTP request line split into method/uri, and the response status code.
    assert_eq!(run("http.request.method == GET"), vec![4, 5]); // request + its retransmission
    assert_eq!(run("http.request.uri contains \"index\""), vec![4, 5]);
    assert_eq!(run("http.response.code == 200"), vec![6]);
    assert!(run("http.response.code >= 400").is_empty());
    // Set membership: `field in { … }` is the union of the equalities, commas optional.
    assert_eq!(run("tcp.port in {443}"), run("tcp.port == 443"));
    assert_eq!(run("tcp.port in {80 443 8080}"), run("tcp.port == 80 or tcp.port == 443 or tcp.port == 8080"));
    assert_eq!(run("tcp.port in {80, 443, 8080}"), run("tcp.port in {80 443 8080}"));
    assert_eq!(run("ip.addr in {8.8.8.8}"), run("ip.addr == 8.8.8.8"));
    assert_eq!(run("frame.number in {1 2 3}"), vec![1, 2, 3]);
    // Numeric ranges are inclusive on both ends.
    assert!(run("tcp.port in {440..450}").contains(&7)); // dst 443 is in range
    assert!(!run("tcp.port in {444..450}").contains(&7)); // 443 is below 444
    assert_eq!(run("frame.number in {6..7}"), vec![6, 7]);
    // CIDR members work inside a set too.
    assert!(run("ip.addr in {10.0.0.0/8 93.184.216.0/24}").contains(&6));
    for bad in [
        "tcp.port ==", "foo.bar", "(tcp", "tcp = 1", "\"x", "dns.qry.name matches \"(\"",
        "tcp.port in {}", "tcp.port in {443", "tcp.port in 443",
    ] {
        assert!(pfilter::parse(bad).is_err(), "{bad} should fail");
    }
    assert!(pfilter::complete("tcp.fl").contains(&"tcp.flags.syn"));
    assert!(pfilter::complete("dns and ip.").contains(&"ip.addr"));
}

#[test]
fn packets_window_tabs_render() {
    use eframe::egui;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cap.pcap");
    let fr = frames();
    std::fs::write(&path, write_pcap(&fr.iter().collect::<Vec<_>>())).unwrap();
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    assert_eq!(app.ext.packets.load(&path).unwrap(), 7);
    app.ext.packets.show = true;
    app.ext.packets.filter = "tcp".into();
    app.ext.packets.sel = Some(3);
    let ctx = egui::Context::default();
    fn walk(s: &egui::Shape, out: &mut String) {
        match s {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let expect: [(u8, &str); 7] = [(0, "GET /index.html"), (1, "93.184.216.34:80"), (2, "192.168.1.10"), (3, "tcp"), (4, "intervals"), (5, "Answers"), (6, "Select a TCP packet")];
    for (tab, want) in expect {
        app.ext.packets.tab = tab;
        let mut text = String::new();
        for _ in 0..2 {
            let big = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1200.0))), ..Default::default() };
            let mut out = ctx.run_ui(big, |ui| app.packets_window(ui.ctx()));
            out.textures_delta.clear();
            text.clear();
            for s in &out.shapes {
                walk(&s.shape, &mut text);
            }
        }
        assert!(text.contains(want), "tab {tab}: missing {want} in\n{text}");
        assert!(text.contains("5 of 7 packets"), "{text}");
    }
    app.ext.packets.filter = "tcp.port ==".into();
    app.ext.packets.refilter();
    assert!(app.ext.packets.filter_err.is_some());
}

// G18 insights + G3 overview: every analysis function on hand-built sessions.
use ferrotap::insights::*;
use ferrotap::model::*;
use std::collections::BTreeMap;

fn sess(id: usize, method: &str, url: &str, status: u16, start: &str, ms: u128) -> SessionRecord {
    SessionRecord {
        id,
        method: method.to_string(),
        url: url.to_string(),
        status,
        http_version: "HTTP/1.1".to_string(),
        response_http_version: "HTTP/1.1".to_string(),
        request_headers: BTreeMap::new(),
        response_headers: BTreeMap::new(),
        request_body: String::new(),
        response_body: String::new(),
        start_time: start.to_string(),
        duration_ms: ms,
        mark: None,
        comment: String::new(),
        response_raw_size: None,
        response_decoded_size: None,
        response_trailers: vec![],
        process_name: Some("chrome.exe".to_string()),
        pid: Some(10),
        client_ip: None,
        remote_ip: None,
        tls_version: None,
        dns_ms: None,
        connect_ms: None,
        tls_ms: None,
        ttfb_ms: None,
        ..Default::default()
    }
}

fn with_resp(mut s: SessionRecord, headers: &[(&str, &str)], body: &str) -> SessionRecord {
    for (k, v) in headers {
        s.response_headers.insert(k.to_string(), v.to_string());
    }
    s.response_body = body.to_string();
    s
}

fn with_req(mut s: SessionRecord, headers: &[(&str, &str)], body: &str) -> SessionRecord {
    for (k, v) in headers {
        s.request_headers.insert(k.to_string(), v.to_string());
    }
    s.request_body = body.to_string();
    s
}

fn refs(v: &[SessionRecord]) -> Vec<&SessionRecord> {
    v.iter().collect()
}

#[test]
fn helpers() {
    assert_eq!(status_class(0), "No response");
    assert_eq!(status_class(204), "2xx Success");
    assert_eq!(status_class(404), "4xx Client error");
    assert_eq!(status_class(503), "5xx Server error");
    assert_eq!(fmt_bytes(512), "512 B");
    assert_eq!(fmt_bytes(2048), "2.0 KB");
    let a = sess(1, "GET", "https://a.example.com:8443/x", 200, "2026-10-16T12:00:00.250+00:00", 5);
    let b = sess(2, "GET", "https://a.example.com/x", 200, "2026-10-16 12:00:01.000", 5);
    assert_eq!(start_epoch_ms(&b).unwrap() - start_epoch_ms(&a).unwrap(), 750);
    assert_eq!(host_name(&a), "a.example.com");
    let mut c = a.clone();
    c.response_raw_size = Some(100);
    c.response_decoded_size = Some(400);
    c.response_body = "x".repeat(400);
    assert_eq!(response_size(&c), 100);
    assert_eq!(decoded_size(&c), 400);
    assert!(is_tunnel(&sess(3, "CONNECT", "tunnel://x.com:443", 200, "", 0)));
}

#[test]
fn overview_counts_bytes_and_times() {
    let mut v = vec![
        with_resp(sess(1, "GET", "https://a.com/", 200, "2026-10-16 12:00:00.000", 100), &[("Content-Type", "text/html; charset=utf-8")], "<html>"),
        with_resp(sess(2, "GET", "https://a.com/app.js", 404, "2026-10-16 12:00:00.500", 300), &[("Content-Type", "application/javascript")], "nope"),
        with_resp(sess(3, "POST", "https://b.com/api", 500, "2026-10-16 12:00:01.000", 2000), &[("Content-Type", "application/json")], "{}"),
        sess(4, "GET", "https://b.com/x", 0, "2026-10-16 12:00:01.100", 40),
    ];
    v[2].request_body = "abcde".into();
    v[3].process_name = None;
    v[0].response_raw_size = Some(2048);
    let o = overview(&refs(&v));
    assert_eq!(o.sessions, 4);
    assert_eq!(o.request_bytes, 5);
    assert_eq!(o.response_bytes, 2048 + 4 + 2);
    assert_eq!(o.span_ms, Some(3000)); // 12:00:00.000 .. 12:00:01.000 + 2000 ms
    assert_eq!(o.first_start.as_deref(), Some("2026-10-16 12:00:00.000"));
    assert_eq!(o.max_ms, 2000);
    assert_eq!(o.max_id, Some(3));
    assert!((o.avg_ms - 610.0).abs() < 0.01);
    assert!(o.by_status.contains(&("4xx Client error".to_string(), 1)));
    assert!(o.by_status.contains(&("No response".to_string(), 1)));
    assert_eq!(o.by_host, vec![("a.com".to_string(), 2), ("b.com".to_string(), 2)]);
    assert!(o.by_content_type.contains(&("text/html".to_string(), 1)));
    assert!(o.by_content_type.contains(&("(none)".to_string(), 1)));
    assert_eq!(o.by_process, vec![("chrome.exe".to_string(), 3), ("(unknown)".to_string(), 1)]);
    assert_eq!(o.duration_histogram.len(), 6);
    assert_eq!(o.duration_histogram[0], ("< 50 ms".to_string(), 1));
    assert_eq!(o.duration_histogram[4], ("1-3 s".to_string(), 1));
    assert_eq!(o.size_histogram[1], ("1-10 KB".to_string(), 1));
    let text = overview_text(&o);
    assert!(text.contains("Sessions: 4"));
    assert!(text.contains("Max duration: 2000 ms (session #3)"));
    assert!(text.contains("a.com\t2"));
    assert_eq!(overview(&[]).sessions, 0);
}

#[test]
fn waterfall_pages_and_slowest_hosts() {
    let v = vec![
        with_resp(sess(1, "GET", "https://site.com/page#top", 200, "2026-10-16 12:00:00.000", 200), &[("content-type", "text/html")], ""),
        with_req(sess(2, "GET", "https://cdn.other.net/lib.js", 200, "2026-10-16 12:00:00.250", 1500), &[("Referer", "https://site.com/page")], ""),
        sess(3, "GET", "https://site.com/style.css", 200, "2026-10-16 12:00:00.300", 100),
        with_resp(sess(4, "GET", "https://site.com/next", 200, "2026-10-16 12:00:05.000", 50), &[("content-type", "text/html")], ""),
        with_req(sess(5, "GET", "https://site.com/img.png", 200, "2026-10-16 12:00:05.100", 20), &[("referer", "https://site.com/next")], ""),
        sess(6, "GET", "https://lonely.org/ping", 200, "2026-10-16 11:59:59.000", 10),
        sess(7, "CONNECT", "tunnel://skip.com:443", 200, "2026-10-16 12:00:00.000", 0),
    ];
    let p = pages(&refs(&v), 1000);
    assert_eq!(p.len(), 3);
    assert_eq!(p[0].url, NO_PAGE);
    assert_eq!(p[0].bars.iter().map(|b| b.id).collect::<Vec<_>>(), vec![6]);
    assert_eq!(p[1].url, "https://site.com/page");
    assert_eq!(p[1].bars.iter().map(|b| b.id).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(p[1].bars[1].offset_ms, 250);
    assert!(p[1].bars[1].slow);
    assert!(!p[1].bars[0].slow);
    assert_eq!(p[1].total_ms, 1750);
    assert_eq!(p[2].bars.iter().map(|b| b.id).collect::<Vec<_>>(), vec![4, 5]);

    let h = slowest_hosts(&refs(&v));
    assert_eq!(h[0].host, "cdn.other.net");
    assert_eq!(h[0].slowest_id, 2);
    let site = h.iter().find(|x| x.host == "site.com").unwrap();
    assert_eq!(site.count, 4);
    assert_eq!(site.max_ms, 200);
    assert_eq!(site.slowest_id, 1);
    assert!((site.avg_ms - 92.5).abs() < 0.01);
    assert!(h.iter().all(|x| x.host != "skip.com"));
}

#[test]
fn error_dashboard_classifies_failures() {
    let mut aborted = sess(6, "GET", "https://e.com/6", 502, "", 1);
    aborted.comment = "Breakpoint aborted: rule".into();
    aborted.response_headers.insert("x-ferrotap-breakpoint".into(), "aborted".into());
    let mut tls = sess(7, "GET", "https://e.com/7", 502, "", 1);
    tls.comment = "TLS handshake failed: invalid peer certificate".into();
    let mut slow = sess(8, "GET", "https://e.com/8", 0, "", 1);
    slow.comment = "upstream timed out".into();
    let v = vec![
        sess(1, "GET", "https://e.com/1", 404, "", 1),
        sess(2, "GET", "https://e.com/2", 403, "", 1),
        with_resp(sess(3, "GET", "https://e.com/3", 500, "", 1), &[("content-type", "text/plain")], "boom"),
        sess(4, "GET", "https://e.com/4", 0, "", 1),
        sess(5, "GET", "https://e.com/5", 504, "", 1),
        aborted,
        tls,
        slow,
        sess(9, "GET", "https://e.com/9", 502, "", 1),
        with_resp(sess(10, "GET", "https://e.com/10", 502, "", 1), &[("server", "nginx")], "bad gateway"),
        sess(11, "GET", "https://e.com/11", 200, "", 1),
    ];
    assert_eq!(failure_kind(&v[3]), Some(FailureKind::FailedConnection));
    assert_eq!(failure_kind(&v[4]), Some(FailureKind::Timeout));
    assert_eq!(failure_kind(&v[5]), Some(FailureKind::Aborted));
    assert_eq!(failure_kind(&v[6]), Some(FailureKind::TlsError));
    assert_eq!(failure_kind(&v[7]), Some(FailureKind::Timeout));
    assert_eq!(failure_kind(&v[8]), Some(FailureKind::FailedConnection)); // proxy-made 502 without headers
    assert_eq!(failure_kind(&v[9]), None); // a server's own 502 page is a 5xx, not a connection failure
    assert_eq!(failure_kind(&v[10]), None);
    assert_eq!(FailureKind::TlsError.title(), "TLS error");

    let e = error_report(&refs(&v));
    assert_eq!(e.client_errors, vec![1, 2]);
    assert_eq!(e.server_errors, vec![3, 5, 6, 7, 9, 10]);
    assert_eq!(e.failed_connections, vec![4, 9]);
    assert_eq!(e.tls_errors, vec![7]);
    assert_eq!(e.timeouts, vec![5, 8]);
    assert_eq!(e.aborted, vec![6]);
    assert_eq!(e.by_status, vec![(403, 1), (404, 1), (500, 1), (502, 4), (504, 1)]);
}

#[test]
fn duplicates_same_method_url_and_body() {
    let mut v = vec![
        with_req(sess(1, "POST", "https://d.com/api", 200, "", 1), &[], "{\"a\":1}"),
        with_req(sess(2, "post", "https://d.com/api", 200, "", 1), &[], "{\"a\":1}"),
        with_req(sess(3, "POST", "https://d.com/api", 200, "", 1), &[], "{\"a\":2}"),
        sess(4, "GET", "https://d.com/x", 200, "", 1),
        sess(5, "GET", "https://d.com/x", 200, "", 1),
        sess(6, "GET", "https://d.com/x", 200, "", 1),
        sess(7, "GET", "https://d.com/y", 200, "", 1),
    ];
    v[4].response_raw_size = Some(100);
    v[5].response_raw_size = Some(50);
    let d = duplicates(&refs(&v));
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].ids, vec![4, 5, 6]);
    assert_eq!(d[0].repeat_bytes, 150);
    assert_eq!(d[1].ids, vec![1, 2]);
    assert_eq!(d[1].method, "POST");
    assert_eq!(d[1].body_len, 7);
}

#[test]
fn cache_analysis_and_explanations() {
    let bare = with_resp(sess(1, "GET", "https://c.com/a.js", 200, "", 1), &[("content-type", "application/javascript")], "x");
    let cc = with_resp(sess(2, "GET", "https://c.com/b.js", 200, "", 1), &[("Cache-Control", "public, max-age=3600, immutable"), ("ETag", "\"v1\"")], "x");
    let lm = with_resp(sess(3, "GET", "https://c.com/c.js", 200, "", 1), &[("Last-Modified", "Wed, 01 Oct 2026 10:00:00 GMT"), ("Vary", "Accept-Encoding")], "x");
    let post = sess(4, "POST", "https://c.com/api", 200, "", 1);
    let notfound = sess(5, "GET", "https://c.com/missing", 404, "", 1);
    let ns = with_resp(sess(6, "GET", "https://c.com/n", 200, "", 1), &[("cache-control", "no-store"), ("Expires", "0"), ("Pragma", "no-cache"), ("Age", "5")], "");
    assert!(lacks_cache_headers(&bare));
    assert!(!lacks_cache_headers(&cc));
    assert!(!lacks_cache_headers(&lm));
    assert!(!is_cache_candidate(&post));
    assert!(!is_cache_candidate(&notfound));

    let x = explain_cache_headers(&cc);
    assert_eq!(x[0], "Cache-Control: public, max-age=3600, immutable");
    assert!(x.iter().any(|l| l.contains("max-age=3600") && l.contains("3600 seconds")));
    assert!(x.iter().any(|l| l.contains("public: shared caches")));
    assert!(x.iter().any(|l| l.contains("immutable:")));
    assert!(x.iter().any(|l| l.starts_with("ETag: \"v1\"") && l.contains("If-None-Match")));
    let y = explain_cache_headers(&lm);
    assert!(y.iter().any(|l| l.contains("If-Modified-Since")));
    assert!(y.iter().any(|l| l.starts_with("Vary: Accept-Encoding")));
    let z = explain_cache_headers(&ns);
    assert!(z.iter().any(|l| l.contains("no-store: must not be stored")));
    assert!(z.iter().any(|l| l.starts_with("Expires: 0")));
    assert!(z.iter().any(|l| l.starts_with("Pragma: no-cache")));
    assert!(z.iter().any(|l| l.starts_with("Age: 5")));
    assert!(explain_cache_headers(&bare)[0].starts_with("No caching headers"));

    let v = vec![cc, lm, bare, post, notfound, ns];
    let a = cache_analysis(&refs(&v));
    assert_eq!(a.iter().map(|e| (e.id, e.missing)).collect::<Vec<_>>(), vec![(1, true), (2, false), (3, false), (6, false)]);
}

#[test]
fn compression_largest_and_images() {
    let big = "a".repeat(5000);
    let mut v = vec![
        with_resp(sess(1, "GET", "https://s.com/app.js", 200, "", 1), &[("Content-Type", "application/javascript")], &big),
        with_resp(sess(2, "GET", "https://s.com/gz.js", 200, "", 1), &[("Content-Type", "text/javascript"), ("Content-Encoding", "gzip")], &big),
        with_resp(sess(3, "GET", "https://s.com/small.css", 200, "", 1), &[("Content-Type", "text/css")], "body{}"),
        with_resp(sess(4, "GET", "https://s.com/data.json", 200, "", 1), &[("Content-Type", "application/json"), ("Content-Encoding", "identity")], &"b".repeat(2000)),
        with_resp(sess(5, "GET", "https://s.com/photo.jpg", 200, "", 1), &[("Content-Type", "image/jpeg")], ""),
        with_resp(sess(6, "GET", "https://s.com/logo.png?v=2", 200, "", 1), &[], ""),
        with_resp(sess(7, "GET", "https://s.com/icon", 200, "", 1), &[("Content-Type", "image/svg+xml")], ""),
    ];
    v[1].response_raw_size = Some(300);
    v[1].response_decoded_size = Some(5000);
    v[4].response_raw_size = Some(300 * 1024);
    v[5].response_raw_size = Some(1000);
    v[6].response_raw_size = Some(2000);
    assert!(is_text_type("text/html"));
    assert!(is_text_type("image/svg+xml"));
    assert!(!is_text_type("image/png"));

    let c = compression_issues(&refs(&v), 1024);
    assert_eq!(c.iter().map(|i| (i.id, i.bytes)).collect::<Vec<_>>(), vec![(1, 5000), (4, 2000), (7, 2000)]);

    let l = largest_payloads(&refs(&v), 3);
    assert_eq!(l.iter().map(|i| i.id).collect::<Vec<_>>(), vec![5, 1, 4]);
    assert_eq!(l[0].bytes, 300 * 1024);
    assert_eq!(largest_payloads(&refs(&v), 100).len(), 7);

    assert!(is_image(&v[5])); // by extension
    let img = image_report(&refs(&v));
    assert_eq!(img.count, 3);
    assert_eq!(img.total_bytes, 300 * 1024 + 1000 + 2000);
    assert_eq!(img.large, 1);
    assert_eq!(img.items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![5, 7, 6]);
    assert_eq!(img.by_type[0], ("image/jpeg".to_string(), 300 * 1024));
    assert!(img.by_type.contains(&("(unknown)".to_string(), 1000)));
}

#[test]
fn header_hygiene_notes_per_host() {
    let good = with_resp(
        sess(1, "GET", "https://good.com/", 200, "", 1),
        &[
            ("content-type", "text/html"),
            ("Strict-Transport-Security", "max-age=63072000"),
            ("Content-Security-Policy", "default-src 'self'; frame-ancestors 'none'"),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
            ("Permissions-Policy", "camera=()"),
            ("Cache-Control", "no-cache"),
        ],
        "",
    );
    let bare_doc = with_resp(sess(2, "GET", "https://bare.com/", 200, "", 1), &[("content-type", "text/html")], "");
    let bare_js = with_resp(sess(3, "GET", "https://bare.com/a.js", 200, "", 1), &[("content-type", "application/javascript"), ("X-Content-Type-Options", "nosniff")], "");
    let plain = with_resp(sess(4, "GET", "http://plain.org/data", 200, "", 1), &[("content-type", "application/json"), ("cache-control", "max-age=1"), ("x-content-type-options", "nosniff")], "");
    let err = sess(5, "GET", "https://broken.com/", 500, "", 1);
    let v = vec![good, bare_doc, bare_js, plain, err];
    let h = header_hygiene(&refs(&v));
    // good.com has everything (frame-ancestors counts for X-Frame-Options); plain.org is HTTP and not HTML; 500s are skipped
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].host, "bare.com");
    assert_eq!(h[0].responses, 2);
    let names: Vec<&str> = h[0].notes.iter().map(|n| n.header).collect();
    assert_eq!(
        names,
        vec!["Strict-Transport-Security", "Content-Security-Policy", "X-Content-Type-Options", "Referrer-Policy", "X-Frame-Options", "Permissions-Policy", "Cache-Control"]
    );
    let hsts = &h[0].notes[0];
    assert_eq!((hsts.missing_in, hsts.applicable), (2, 2));
    let xcto = h[0].notes.iter().find(|n| n.header == "X-Content-Type-Options").unwrap();
    assert_eq!((xcto.missing_in, xcto.applicable, xcto.example_id), (1, 2, 2));
    let csp = h[0].notes.iter().find(|n| n.header == "Content-Security-Policy").unwrap();
    assert_eq!(csp.applicable, 1);
    assert!(!csp.explanation.is_empty());
}

#[test]
fn cookies_per_domain() {
    let c = parse_set_cookie("sid=abc123; Path=/app; Domain=.Example.com; Secure; HttpOnly; SameSite=Lax; Max-Age=3600", "www.example.com", 9).unwrap();
    assert_eq!(c.name, "sid");
    assert_eq!(c.size, 9);
    assert_eq!(c.domain, "example.com");
    assert_eq!(c.path, "/app");
    assert!(c.secure && c.http_only);
    assert_eq!(c.same_site.as_deref(), Some("Lax"));
    assert_eq!(c.expiry.as_deref(), Some("Max-Age=3600 s"));
    assert_eq!(c.session_id, 9);
    let s = parse_set_cookie("theme=dark", "www.example.com", 1).unwrap();
    assert_eq!((s.domain.as_str(), s.secure, s.expiry.clone()), ("www.example.com", false, None));
    let e = parse_set_cookie("t=1; Expires=Wed, 21 Oct 2026 07:28:00 GMT", "h", 1).unwrap();
    assert_eq!(e.expiry.as_deref(), Some("Wed, 21 Oct 2026 07:28:00 GMT"));
    assert!(parse_set_cookie("garbage", "h", 1).is_none());

    let v = vec![
        with_resp(sess(1, "GET", "https://www.example.com/", 200, "", 1), &[("Set-Cookie", "sid=abc123; Domain=example.com; Secure; HttpOnly\nlang=en")], ""),
        with_resp(sess(2, "GET", "https://www.example.com/x", 200, "", 1), &[("set-cookie", "lang=fr")], ""),
        with_req(sess(3, "GET", "https://www.example.com/y", 200, "", 1), &[("Cookie", "sid=abc123; lang=fr; lang=fr")], ""),
        with_req(sess(4, "GET", "https://tracker.net:443/p", 200, "", 1), &[("cookie", "uid=42")], ""),
    ];
    let d = cookie_overview(&refs(&v));
    assert_eq!(d.iter().map(|x| x.domain.as_str()).collect::<Vec<_>>(), vec!["example.com", "tracker.net", "www.example.com"]);
    assert_eq!(d[0].set.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["sid"]);
    let www = &d[2];
    assert_eq!(www.set.len(), 1);
    assert_eq!(www.set[0].name, "lang");
    assert_eq!(www.set[0].size, 6); // the later "lang=fr" replaced "lang=en"
    assert_eq!(www.set[0].session_id, 2);
    assert_eq!(www.set_bytes, 6);
    assert_eq!(www.sent_names, vec!["lang".to_string(), "sid".to_string()]);
    assert_eq!(www.max_sent_header, "sid=abc123; lang=fr; lang=fr".len());
    assert_eq!(d[1].sent_names, vec!["uid".to_string()]);
    assert!(d[1].set.is_empty());
}

#[test]
fn third_party_summary() {
    assert_eq!(site_of("cdn.static.example.com"), "example.com");
    assert_eq!(site_of("www.shop.co.uk:8080"), "shop.co.uk");
    assert_eq!(site_of("localhost"), "localhost");
    assert_eq!(site_of("192.168.1.10"), "192.168.1.10");
    let v = vec![
        sess(1, "GET", "https://api.example.com/x", 200, "2026-10-16 12:00:00.000", 1),
        with_resp(sess(2, "GET", "https://www.example.com/", 200, "2026-10-16 12:00:01.000", 1), &[("content-type", "text/html")], ""),
        sess(3, "GET", "https://img.example.com/a.png", 200, "2026-10-16 12:00:02.000", 1),
        sess(4, "GET", "https://cdn.vendor.net/lib.js", 200, "2026-10-16 12:00:02.000", 1),
        sess(5, "GET", "https://cdn.vendor.net/lib2.js", 200, "2026-10-16 12:00:02.000", 1),
        sess(6, "GET", "https://ads.track.io/p", 200, "2026-10-16 12:00:02.000", 1),
    ];
    assert_eq!(main_site(&refs(&v)).as_deref(), Some("example.com"));
    let mut v2 = v.clone();
    v2.remove(1);
    assert_eq!(main_site(&refs(&v2)).as_deref(), Some("example.com")); // most requested site
    let mut with_size = v.clone();
    with_size[3].response_raw_size = Some(1000);
    let t = third_party(&refs(&with_size), "example.com");
    assert_eq!(t.iter().map(|x| (x.host.as_str(), x.count)).collect::<Vec<_>>(), vec![("cdn.vendor.net", 2), ("ads.track.io", 1)]);
    assert_eq!(t[0].site, "vendor.net");
    assert_eq!(t[0].bytes, 1000);
    assert_eq!(t[0].first_id, 4);
    // another main site turns example.com into the third party
    let t2 = third_party(&refs(&v), "www.vendor.net");
    assert!(t2.iter().any(|x| x.host == "www.example.com"));
    assert!(t2.iter().all(|x| x.site != "vendor.net"));
}

#[test]
fn overview_units_and_phase_averages() {
    use ferrotap::insights::*;
    assert_eq!(fmt_time(1500.0, TimeUnit::Ms), "1500 ms");
    assert_eq!(fmt_time(1500.0, TimeUnit::Seconds), "1.50 s");
    assert_eq!(fmt_size(2048, SizeUnit::Auto), "2.0 KB");
    assert_eq!(fmt_size(2048, SizeUnit::Bytes), "2048 B");
    assert_eq!(fmt_size(1048576, SizeUnit::Mb), "1.000 MB");
    let a = ferrotap::model::SessionRecord { dns_ms: Some(10), connect_ms: Some(20), tls_ms: Some(30), ttfb_ms: Some(40), duration_ms: 200, ..Default::default() };
    let b = ferrotap::model::SessionRecord { ttfb_ms: Some(60), duration_ms: 100, ..Default::default() };
    let p = phase_averages(&[&a, &b]);
    assert_eq!(p[0], ("DNS", 10.0, 1));
    assert_eq!(p[3], ("Server (TTFB)", 50.0, 2));
    assert_eq!(p[4], ("Receive", 70.0, 2)); // (200-100 + 100-60) / 2
    assert_eq!(OverviewPrefs::default().phases, [true; 5]);
}

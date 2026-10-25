//! G3 inspectors: query / cookie / form parsing, pretty printers, search, sizes, timings.
use ferrotap::inspect::*;
use ferrotap::model::SessionRecord;

#[test]
fn query_params_are_decoded_in_order() {
    let q = parse_query("https://x.io/s?q=rust+lang&n=1&empty=&flag&enc=a%2Fb%20c#frag=1");
    assert_eq!(
        q,
        vec![
            ("q".into(), "rust lang".into()),
            ("n".into(), "1".into()),
            ("empty".into(), "".into()),
            ("flag".into(), "".into()),
            ("enc".into(), "a/b c".into()),
        ]
    );
    assert!(parse_query("http://x.io/").is_empty());
}

#[test]
fn url_decode_keeps_invalid_escapes() {
    assert_eq!(url_decode("100%", false), "100%");
    assert_eq!(url_decode("%zz%41", false), "%zzA");
    assert_eq!(url_decode("a+b", false), "a+b");
    assert_eq!(url_decode("%E2%82%AC", false), "\u{20ac}");
    assert_eq!(url_encode("a b/\u{e9}"), "a%20b%2F%C3%A9");
}

#[test]
fn request_cookie_pairs() {
    let c = parse_cookie_header("sid=abc; theme=dark ;  lone; x=a=b");
    assert_eq!(
        c,
        vec![("sid".into(), "abc".into()), ("theme".into(), "dark".into()), ("lone".into(), "".into()), ("x".into(), "a=b".into())]
    );
}

#[test]
fn set_cookie_all_attributes() {
    let c = parse_set_cookie("id=a3fWa; Expires=Thu, 31 Oct 2030 07:28:00 GMT; Max-Age=3600; Domain=.example.com; Path=/app; Secure; HttpOnly; SameSite=Lax").unwrap();
    assert_eq!(c.name, "id");
    assert_eq!(c.value, "a3fWa");
    assert_eq!(c.expires.as_deref(), Some("Thu, 31 Oct 2030 07:28:00 GMT"));
    assert_eq!(c.max_age.as_deref(), Some("3600"));
    assert_eq!(c.domain.as_deref(), Some(".example.com"));
    assert_eq!(c.path.as_deref(), Some("/app"));
    assert!(c.secure && c.http_only);
    assert_eq!(c.same_site.as_deref(), Some("Lax"));

    let plain = parse_set_cookie("k=v").unwrap();
    assert!(!plain.secure && !plain.http_only && plain.domain.is_none());
    assert!(parse_set_cookie("   ").is_none());
}

#[test]
fn set_cookie_header_splitting_respects_expires_commas() {
    let v = "a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/, b=2; HttpOnly\nc=3";
    let parts = split_set_cookie_header(v);
    assert_eq!(parts, vec!["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/", "b=2; HttpOnly", "c=3"]);
    let mut h = std::collections::BTreeMap::new();
    h.insert("set-cookie".to_string(), v.to_string());
    h.insert("content-type".to_string(), "text/html".to_string());
    let cs = response_set_cookies(&h);
    assert_eq!(cs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
    assert!(cs[1].http_only);
}

fn form_session(ct: &str, body: &str) -> SessionRecord {
    let mut s = SessionRecord { method: "POST".into(), url: "http://h/f".into(), request_body: body.into(), ..Default::default() };
    s.request_headers.insert("Content-Type".into(), ct.into());
    s
}

#[test]
fn urlencoded_form_fields() {
    let s = form_session("application/x-www-form-urlencoded; charset=UTF-8", "user=jo+doe&pass=p%40ss&remember=on");
    let f = parse_form(&s).unwrap();
    assert_eq!(f.len(), 3);
    assert_eq!(f[0].name, "user");
    assert_eq!(f[0].value, "jo doe");
    assert_eq!(f[1].value, "p@ss");
    assert!(f.iter().all(|p| p.filename.is_none()));
    assert!(parse_form(&form_session("application/json", "{}")).is_none());
}

#[test]
fn multipart_form_parts() {
    let body = "--XyZ\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nHello\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nline1\r\nline2\r\n--XyZ--\r\n";
    let s = form_session("multipart/form-data; boundary=\"XyZ\"", body);
    let f = parse_form(&s).unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].name, "title");
    assert_eq!(f[0].value, "Hello");
    assert_eq!(f[0].size, 5);
    assert_eq!(f[1].name, "file");
    assert_eq!(f[1].filename.as_deref(), Some("a.txt"));
    assert_eq!(f[1].content_type.as_deref(), Some("text/plain"));
    assert_eq!(f[1].size, "line1\r\nline2".len());
    assert!(parse_multipart(body, "multipart/form-data").is_empty(), "no boundary -> no parts");
}

#[test]
fn xml_is_indented() {
    let x = pretty_markup("<?xml version=\"1.0\"?><root a=\"1>2\"><item>one</item><item><sub/></item><!-- c --></root>", false);
    assert_eq!(
        x,
        "<?xml version=\"1.0\"?>\n<root a=\"1>2\">\n  <item>one</item>\n  <item>\n    <sub/>\n  </item>\n  <!-- c -->\n</root>"
    );
}

#[test]
fn html_knows_void_and_script() {
    let h = pretty_markup("<!DOCTYPE html><html><head><meta charset=utf-8><script>if (a < b) { x(); }</script></head><body><p>Hi<br>there</p></body></html>", true);
    let lines: Vec<&str> = h.lines().collect();
    assert_eq!(lines[0], "<!DOCTYPE html>");
    assert_eq!(lines[1], "<html>");
    assert_eq!(lines[2], "  <head>");
    assert_eq!(lines[3], "    <meta charset=utf-8>");
    assert_eq!(lines[4], "    <script>if (a < b) { x(); }</script>");
    let multi = pretty_markup("<div><script>
var a = 1;
  if (a < 2) { go(); }
</script></div>", true);
    assert_eq!(multi, "<div>
  <script>
    var a = 1;
    if (a < 2) { go(); }
  </script>
</div>");
    assert!(h.contains("      <br>\n"), "{h}");
    assert!(h.ends_with("</html>"));
}

#[test]
fn js_is_indented() {
    let js = pretty_js("function f(a){for(var i=0;i<a;i++){console.log(\"{;}\");}return a;}// done");
    assert_eq!(js, "function f(a){\n  for(var i=0;i<a;i++){\n    console.log(\"{;}\");\n  }\n  return a;\n}\n// done");
    let js2 = pretty_js("var o = {a: 1, b: {c: 2}};\n/* x; { */ go();");
    assert!(js2.contains("};"), "{js2}");
    assert!(js2.contains("/* x; { */ go();"), "{js2}");
}

#[test]
fn search_finds_all_matches_case_insensitive() {
    let m = find_matches("Foo foo FOO bar", "foo", false);
    assert_eq!(m, vec![0..3, 4..7, 8..11]);
    assert_eq!(find_matches("Foo foo", "foo", true), vec![4..7]);
    assert!(find_matches("abc", "", false).is_empty());
    assert_eq!(find_matches("\u{e9}t\u{e9} ETE", "te", false), vec![7..9]);
}

#[test]
fn sizes_and_timings() {
    let mut s = SessionRecord { method: "GET".into(), url: "http://h/p".into(), http_version: "HTTP/1.1".into(), response_http_version: "HTTP/1.1".into(), status: 200, ..Default::default() };
    s.request_headers.insert("Host".into(), "h".into());
    s.response_body = "hello".into();
    s.response_raw_size = Some(25);
    s.response_decoded_size = Some(5);
    let z = sizes(&s);
    // "GET /p HTTP/1.1\r\n" (17) + "Host: h\r\n" (9) + "\r\n" (2)
    assert_eq!(z.request_header_bytes, 28);
    assert_eq!(z.response_body_bytes, 5);
    assert_eq!(z.response_raw_size, Some(25));

    s.duration_ms = 100;
    s.connect_ms = Some(10);
    s.tls_ms = Some(20);
    s.ttfb_ms = Some(30);
    let p = timing_phases(&s);
    assert_eq!(p, vec![("TCP connect", 0, 10), ("TLS handshake", 10, 20), ("Server (TTFB)", 30, 30), ("Receive", 60, 40)]);
    s.dns_ms = Some(5);
    assert_eq!(timing_phases(&s)[0], ("DNS", 0, 5));
}

#[test]
fn file_names_and_saving() {
    assert_eq!(suggested_file_name("https://h/a/b/data?x=1", "application/json", 7, "response"), "data.json");
    assert_eq!(suggested_file_name("https://h/", "text/html", 7, "response"), "session7_response.html");
    assert_eq!(suggested_file_name("https://h/img/logo.png", "image/png", 1, "response"), "logo.png");
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("body.txt");
    assert_eq!(save_text(&p, "abc").unwrap(), 3);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "abc");
    assert!(save_text(std::path::Path::new(""), "x").is_err());
}

#[test]
fn downloads_dir_env_override() {
    // Only this test touches the variable.
    std::env::set_var("FERROTAP_DOWNLOADS_DIR", "Z:\\tmp\\dl");
    assert_eq!(downloads_dir(), std::path::PathBuf::from("Z:\\tmp\\dl"));
    std::env::remove_var("FERROTAP_DOWNLOADS_DIR");
    assert!(downloads_dir().ends_with("Downloads") || downloads_dir() == std::path::PathBuf::from("."));
}

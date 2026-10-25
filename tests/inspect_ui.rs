//! G3 inspectors: headless egui smoke test that every inspector view renders without panicking,
//! including search highlighting, scroll-to-match and the decoded/original toggle.
use ferrotap::inspect::*;
use ferrotap::model::SessionRecord;

fn session() -> SessionRecord {
    let mut s = SessionRecord {
        id: 3,
        method: "POST".into(),
        url: "https://h.example/api/login?next=%2Fhome&x=1".into(),
        status: 200,
        http_version: "HTTP/1.1".into(),
        response_http_version: "HTTP/1.1".into(),
        request_body: "user=jo&pass=x".into(),
        response_body: "<html><body><p>hello hello</p></body></html>".into(),
        duration_ms: 120,
        connect_ms: Some(10),
        tls_ms: Some(30),
        ttfb_ms: Some(50),
        response_raw_size: Some(40),
        response_decoded_size: Some(46),
        response_trailers: vec![("Signature".into(), "abc".into())],
        ..Default::default()
    };
    s.request_headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
    s.request_headers.insert("Cookie".into(), "a=1; b=2".into());
    s.response_headers.insert("Set-Cookie".into(), "sid=9; Path=/; HttpOnly; SameSite=Strict".into());
    s.response_headers.insert("Content-Encoding".into(), "gzip".into());
    s
}

#[test]
fn all_views_render_headless() {
    let ctx = egui::Context::default();
    let s = session();
    let mut st = State::default();
    st.resp.search = "hello".into();
    st.resp.scroll_pending = true;
    for frame in 0..3 {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            params_view(ui, &s);
            cookies_view(ui, &s, false);
            cookies_view(ui, &s, true);
            forms_view(ui, &s);
            trailers_view(ui, &s);
            timings_view(ui, &s);
            sizes_view(ui, &s);
            let body = encoding_bar(ui, &mut st.resp, s.response_header("content-encoding"), s.response_header("content-type"), &s.response_body);
            let pretty = pretty_markup(&body, true);
            body_view(ui, &mut st.resp, &mut st.wrap, "t_body", &pretty, (s.id, "x.html".into()));
            st.wrap = frame % 2 == 0;
        });
        out.drop_without_applying_deltas();
    }
    assert_eq!(find_matches(&s.response_body, &st.resp.search, false).len(), 2);
    // The save path defaults to the downloads folder + suggested name and is not written unless clicked.
    assert!(st.resp.save_path.ends_with("x.html"), "{}", st.resp.save_path);
    assert!(!std::path::Path::new(&st.resp.save_path).exists() || st.resp.save_msg.is_empty());
}

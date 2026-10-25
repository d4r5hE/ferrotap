//! G18 session report: HTML, Markdown and PDF from chosen sessions.
use ferrotap::model::SessionRecord;
use ferrotap::report::*;

fn rows() -> Vec<SessionRecord> {
    let mut a = SessionRecord { id: 1, method: "GET".into(), url: "https://a.example/<x>|y".into(), status: 200, duration_ms: 120, ..Default::default() };
    a.response_headers.insert("content-type".into(), "text/html; charset=utf-8".into());
    a.set_response_body(b"hello");
    a.ttfb_ms = Some(80);
    let b = SessionRecord { id: 2, method: "POST".into(), url: "https://b.example/api".into(), status: 500, duration_ms: 900, comment: "server broke (see logs)".into(), ..Default::default() };
    let c = SessionRecord { id: 3, method: "GET".into(), url: "http://127.0.0.1:9/gone".into(), status: 502, comment: "Upstream failed: connection refused".into(), ..Default::default() };
    vec![a, b, c]
}

#[test]
fn report_summary() {
    let r = build("T", &rows());
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.total_ms, 1020);
    assert_eq!(r.slowest, Some((2, 900)));
    assert_eq!(r.by_class, vec![("2xx".to_string(), 1), ("5xx".to_string(), 2)]);
    assert!(r.errors.iter().any(|(l, ids)| l.starts_with("Server errors") && ids.contains(&2)));
    assert!(r.errors.iter().any(|(l, ids)| l == "Failed connections" && ids == &vec![3]));
    assert_eq!(r.rows[0].content_type, "text/html");
    assert_eq!(r.rows[0].timings, "TTFB 80");
}

#[test]
fn markdown_and_html_are_escaped() {
    let r = build("My <report>", &rows());
    let md = to_markdown(&r);
    assert!(md.starts_with("# My <report>"));
    assert!(md.contains("https://a.example/<x>\\|y"));
    assert!(md.contains("## Notes") && md.contains("#2: server broke"));
    let html = to_html(&r);
    assert!(html.contains("<title>My &lt;report&gt;</title>"));
    assert!(html.contains("https://a.example/&lt;x&gt;|y"));
    assert!(!html.contains("<x>"));
}

#[test]
fn pdf_is_well_formed() {
    // enough rows for several pages
    let mut many = Vec::new();
    for i in 0..200 {
        many.push(SessionRecord { id: i + 1, method: "GET".into(), url: format!("https://e.example/{i}/(paren)\\back"), status: 200, ..Default::default() });
    }
    let pdf = to_pdf(&build("Big", &many));
    let s = String::from_utf8(pdf.clone()).unwrap();
    assert!(s.starts_with("%PDF-1.4") && s.trim_end().ends_with("%%EOF"));
    let pages = s.matches("/Type /Page ").count();
    assert!(pages >= 3, "{pages} pages");
    assert!(s.contains(&format!("/Count {pages}")));
    assert!(s.contains("\\(paren\\)\\\\back"));
    // xref offsets point at the objects
    let xref_at: usize = s.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
    assert!(s[xref_at..].starts_with("xref"));
    let first_off: usize = s[xref_at..].lines().nth(3).unwrap()[..10].parse().unwrap();
    assert!(s[first_off..].starts_with("1 0 obj"));
}

#[test]
fn write_all_formats() {
    let dir = tempfile::tempdir().unwrap();
    for f in ReportFormat::ALL {
        let p = dir.path().join(format!("r.{}", f.ext()));
        assert_eq!(write_report(&p, f, "T", &rows()).unwrap(), 3);
        assert!(std::fs::metadata(&p).unwrap().len() > 100);
    }
}

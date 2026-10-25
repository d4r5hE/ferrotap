//! G18 session report: a summary of chosen sessions (counts, timings, errors, notes) as HTML,
//! Markdown or PDF. Pure functions; the File > Session report window lives in data_io.rs.
use std::fmt::Write as _;
use std::path::Path;

use crate::model::SessionRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReportFormat {
    #[default]
    Html,
    Markdown,
    Pdf,
}

impl ReportFormat {
    pub const ALL: [ReportFormat; 3] = [ReportFormat::Html, ReportFormat::Markdown, ReportFormat::Pdf];
    pub fn ext(self) -> &'static str {
        match self {
            ReportFormat::Html => "html",
            ReportFormat::Markdown => "md",
            ReportFormat::Pdf => "pdf",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ReportFormat::Html => "HTML (.html)",
            ReportFormat::Markdown => "Markdown (.md)",
            ReportFormat::Pdf => "PDF (.pdf)",
        }
    }
}

/// One session line of the report.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: usize,
    pub method: String,
    pub status: u16,
    pub url: String,
    pub duration_ms: u64,
    pub size: usize,
    pub content_type: String,
    /// DNS / connect / TLS / TTFB in ms when measured.
    pub timings: String,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub title: String,
    pub generated: String,
    pub rows: Vec<Row>,
    /// (status class label, count), e.g. ("2xx", 10).
    pub by_class: Vec<(String, usize)>,
    pub total_bytes: usize,
    pub total_ms: u64,
    pub slowest: Option<(usize, u64)>,
    /// (label, session ids) of each non-empty error group.
    pub errors: Vec<(String, Vec<usize>)>,
}

fn timings(s: &SessionRecord) -> String {
    let parts: Vec<String> = [("DNS", s.dns_ms), ("connect", s.connect_ms), ("TLS", s.tls_ms), ("TTFB", s.ttfb_ms)]
        .iter()
        .filter_map(|(n, v)| v.map(|v| format!("{n} {v}")))
        .collect();
    parts.join(", ")
}

/// Build the report data for `sessions` (in the given order).
pub fn build(title: &str, sessions: &[SessionRecord]) -> Report {
    let mut r = Report { title: title.to_string(), generated: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), ..Default::default() };
    let mut classes: std::collections::BTreeMap<String, usize> = Default::default();
    for s in sessions {
        let size = s.response_raw_size.unwrap_or_else(|| s.response_bytes().len());
        let dur = s.duration_ms as u64;
        r.total_bytes += size;
        r.total_ms += dur;
        if r.slowest.is_none_or(|(_, d)| dur > d) {
            r.slowest = Some((s.id, dur));
        }
        let class = if s.status == 0 { "no response".to_string() } else { format!("{}xx", s.status / 100) };
        *classes.entry(class).or_default() += 1;
        r.rows.push(Row {
            id: s.id,
            method: s.method.clone(),
            status: s.status,
            url: s.url.clone(),
            duration_ms: dur,
            size,
            content_type: s.response_header("content-type").unwrap_or("").split(';').next().unwrap_or("").trim().to_string(),
            timings: timings(s),
            comment: s.comment.clone(),
        });
    }
    r.by_class = classes.into_iter().collect();
    let refs: Vec<&SessionRecord> = sessions.iter().collect();
    let e = crate::insights::error_report(&refs);
    for (label, ids) in [
        ("Client errors (4xx)", e.client_errors),
        ("Server errors (5xx)", e.server_errors),
        ("Failed connections", e.failed_connections),
        ("TLS errors", e.tls_errors),
        ("Timeouts", e.timeouts),
        ("Aborted", e.aborted),
    ] {
        if !ids.is_empty() {
            r.errors.push((label.to_string(), ids));
        }
    }
    r
}

fn kb(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn ids(v: &[usize]) -> String {
    v.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(", ")
}

fn summary_lines(r: &Report) -> Vec<String> {
    let mut v = vec![
        format!("Sessions: {}", r.rows.len()),
        format!("Status: {}", r.by_class.iter().map(|(c, n)| format!("{c}: {n}")).collect::<Vec<_>>().join(", ")),
        format!("Total response size: {}", kb(r.total_bytes)),
        format!("Total time: {} ms", r.total_ms),
    ];
    if let Some((id, ms)) = r.slowest {
        v.push(format!("Slowest: #{id} ({ms} ms)"));
    }
    v
}

fn notes(r: &Report) -> Vec<&Row> {
    r.rows.iter().filter(|x| !x.comment.trim().is_empty()).collect()
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\r', '\n'], " ")
}

pub fn to_markdown(r: &Report) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# {}\n\nGenerated {} by Ferrotap.\n\n## Summary\n", r.title, r.generated);
    for l in summary_lines(r) {
        let _ = writeln!(o, "- {l}");
    }
    o.push_str("\n## Errors\n\n");
    if r.errors.is_empty() {
        o.push_str("No errors.\n");
    }
    for (label, v) in &r.errors {
        let _ = writeln!(o, "- **{label}** ({}): {}", v.len(), ids(v));
    }
    o.push_str("\n## Sessions\n\n| # | Method | Status | URL | Time (ms) | Size | Type | Timings (ms) |\n|---|---|---|---|---|---|---|---|\n");
    for x in &r.rows {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            x.id,
            md_cell(&x.method),
            x.status,
            md_cell(&x.url),
            x.duration_ms,
            kb(x.size),
            md_cell(&x.content_type),
            md_cell(&x.timings)
        );
    }
    let n = notes(r);
    if !n.is_empty() {
        o.push_str("\n## Notes\n\n");
        for x in n {
            let _ = writeln!(o, "- #{}: {}", x.id, md_cell(&x.comment));
        }
    }
    o
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

pub fn to_html(r: &Report) -> String {
    let mut o = String::new();
    let _ = write!(
        o,
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{t}</title>\n<style>\n\
         body{{font:14px system-ui,sans-serif;margin:24px;color:#222;background:#fff}}\n\
         table{{border-collapse:collapse;width:100%}}td,th{{border:1px solid #ccc;padding:4px 6px;text-align:left;vertical-align:top}}\n\
         th{{background:#f2f2f2}}td.u{{word-break:break-all}}.e{{color:#b00020}}.m{{color:#666}}\n\
         </style></head><body>\n<h1>{t}</h1>\n<p class=\"m\">Generated {g} by Ferrotap.</p>\n<h2>Summary</h2>\n<ul>\n",
        t = esc(&r.title),
        g = esc(&r.generated)
    );
    for l in summary_lines(r) {
        let _ = writeln!(o, "<li>{}</li>", esc(&l));
    }
    o.push_str("</ul>\n<h2>Errors</h2>\n");
    if r.errors.is_empty() {
        o.push_str("<p>No errors.</p>\n");
    } else {
        o.push_str("<ul>\n");
        for (label, v) in &r.errors {
            let _ = writeln!(o, "<li class=\"e\"><b>{}</b> ({}): {}</li>", esc(label), v.len(), esc(&ids(v)));
        }
        o.push_str("</ul>\n");
    }
    o.push_str("<h2>Sessions</h2>\n<table>\n<tr><th>#</th><th>Method</th><th>Status</th><th>URL</th><th>Time (ms)</th><th>Size</th><th>Type</th><th>Timings (ms)</th></tr>\n");
    for x in &r.rows {
        let cls = if x.status == 0 || x.status >= 400 { " class=\"e\"" } else { "" };
        let _ = writeln!(
            o,
            "<tr><td>{}</td><td>{}</td><td{cls}>{}</td><td class=\"u\">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            x.id,
            esc(&x.method),
            x.status,
            esc(&x.url),
            x.duration_ms,
            kb(x.size),
            esc(&x.content_type),
            esc(&x.timings)
        );
    }
    o.push_str("</table>\n");
    let n = notes(r);
    if !n.is_empty() {
        o.push_str("<h2>Notes</h2>\n<ul>\n");
        for x in n {
            let _ = writeln!(o, "<li>#{}: {}</li>", x.id, esc(&x.comment));
        }
        o.push_str("</ul>\n");
    }
    o.push_str("</body></html>\n");
    o
}

/// Plain text lines (used for the PDF), at most `width` characters each.
pub fn to_text_lines(r: &Report, width: usize) -> Vec<String> {
    let mut v = vec![r.title.clone(), format!("Generated {} by Ferrotap.", r.generated), String::new(), "SUMMARY".into()];
    v.extend(summary_lines(r).into_iter().map(|l| format!("  {l}")));
    v.push(String::new());
    v.push("ERRORS".into());
    if r.errors.is_empty() {
        v.push("  No errors.".into());
    }
    for (label, i) in &r.errors {
        v.push(format!("  {label} ({}): {}", i.len(), ids(i)));
    }
    v.push(String::new());
    v.push("SESSIONS".into());
    for x in &r.rows {
        v.push(format!("  #{} {} {} {} ms {}  {}", x.id, x.status, x.method, x.duration_ms, kb(x.size), x.url));
        if !x.timings.is_empty() {
            v.push(format!("      timings: {}", x.timings));
        }
    }
    let n = notes(r);
    if !n.is_empty() {
        v.push(String::new());
        v.push("NOTES".into());
        for x in n {
            v.push(format!("  #{}: {}", x.id, x.comment));
        }
    }
    // wrap long lines
    let mut out = Vec::new();
    for l in v {
        let chars: Vec<char> = l.chars().collect();
        if chars.len() <= width {
            out.push(l);
            continue;
        }
        let mut first = true;
        for chunk in chars.chunks(width - 6) {
            let s: String = chunk.iter().collect();
            out.push(if first { s } else { format!("      {s}") });
            first = false;
        }
    }
    out
}

fn pdf_str(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            c if (' '..='~').contains(&c) => o.push(c),
            _ => o.push('?'),
        }
    }
    o
}

/// A minimal text-only PDF (A4, Courier 9pt), one or more pages.
pub fn to_pdf(r: &Report) -> Vec<u8> {
    const PER_PAGE: usize = 72;
    let lines = to_text_lines(r, 100);
    let pages: Vec<&[String]> = if lines.is_empty() { vec![&[][..]] } else { lines.chunks(PER_PAGE).collect() };
    // objects: 1 catalog, 2 pages, 3 font, then (page, content) per page
    let mut objs: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 4 + i * 2)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into());
    for (i, page) in pages.iter().enumerate() {
        let mut c = String::from("BT /F1 9 Tf 11 TL 40 800 Td\n");
        for l in page.iter() {
            let _ = writeln!(c, "({}) '", pdf_str(l));
        }
        c.push_str("ET\n");
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + i * 2
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{}endstream", c.len(), c));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        let _ = write!(out, "{} 0 obj\n{}\nendobj\n", i + 1, o);
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for off in offsets {
        let _ = writeln!(out, "{off:010} 00000 n ");
    }
    let _ = write!(out, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1);
    out.into_bytes()
}

/// Render and write the report; returns the number of sessions in it.
pub fn write_report(path: &Path, fmt: ReportFormat, title: &str, sessions: &[SessionRecord]) -> Result<usize, String> {
    let r = build(title, sessions);
    let bytes = match fmt {
        ReportFormat::Html => to_html(&r).into_bytes(),
        ReportFormat::Markdown => to_markdown(&r).into_bytes(),
        ReportFormat::Pdf => to_pdf(&r),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(r.rows.len())
}

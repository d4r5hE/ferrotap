ROOT = r"D:\Projects\Ferrotap"
M = ROOT + r"\src\model.rs"
U = ROOT + r"\src\ui.rs"

MODEL_ADD = r'''

// ===================== Composer: build a request and dispatch it through the capture proxy =====================
/// A request built in the Composer panel.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ComposerRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Result of dispatching a ComposerRequest (never panics; errors land in `error`).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ComposerResponse {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub elapsed_ms: u128,
    pub error: String,
}

/// Parse "Name: value" lines (one per line) into header pairs.
pub fn parse_header_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            if k.is_empty() { None } else { Some((k.to_string(), v.trim().to_string())) }
        })
        .collect()
}

impl ComposerRequest {
    /// Authority (host[:port]) of an http:// URL.
    fn authority(&self) -> Result<String, String> {
        let rest = self.url.trim().strip_prefix("http://").ok_or_else(|| {
            "only http:// URLs can be dispatched for now (https needs CONNECT, next part)".to_string()
        })?;
        let authority = rest.split('/').next().unwrap_or("").to_string();
        if authority.is_empty() {
            return Err("URL has no host".into());
        }
        Ok(authority)
    }

    /// Proxy-style request bytes (absolute-form request line) with auto Host / Content-Length / Connection.
    pub fn to_wire(&self) -> Result<Vec<u8>, String> {
        let authority = self.authority()?;
        let method = if self.method.trim().is_empty() { "GET" } else { self.method.trim() }.to_uppercase();
        let has = |n: &str| self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
        let mut s = format!("{} {} HTTP/1.1\r\n", method, self.url.trim());
        if !has("host") {
            s.push_str(&format!("Host: {authority}\r\n"));
        }
        for (k, v) in &self.headers {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        if !has("content-length") && (!self.body.is_empty() || matches!(method.as_str(), "POST" | "PUT" | "PATCH")) {
            s.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        if !has("connection") {
            s.push_str("Connection: close\r\n");
        }
        s.push_str("\r\n");
        let mut b = s.into_bytes();
        b.extend_from_slice(self.body.as_bytes());
        Ok(b)
    }
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let Some(p) = raw[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let size_s = String::from_utf8_lossy(&raw[i..i + p]).to_string();
        let Ok(n) = usize::from_str_radix(size_s.split(';').next().unwrap_or("").trim(), 16) else { break };
        i += p + 2;
        if n == 0 { break; }
        if i + n > raw.len() {
            out.extend_from_slice(&raw[i..]);
            break;
        }
        out.extend_from_slice(&raw[i..i + n]);
        i += n + 2;
    }
    out
}

/// Send `req` to the capture proxy at `proxy` (e.g. "127.0.0.1:8897") and parse the response.
pub fn composer_send(req: &ComposerRequest, proxy: &str) -> ComposerResponse {
    use std::io::{Read, Write};
    let t0 = std::time::Instant::now();
    let fail = |e: String| ComposerResponse { error: e, elapsed_ms: t0.elapsed().as_millis(), ..Default::default() };
    let wire = match req.to_wire() {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    let addr = match proxy.parse::<std::net::SocketAddr>() {
        Ok(a) => a,
        Err(e) => return fail(format!("bad proxy address {proxy}: {e}")),
    };
    let mut st = match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => return fail(format!("cannot reach proxy {proxy}: {e}")),
    };
    let _ = st.set_read_timeout(Some(std::time::Duration::from_secs(20)));
    if let Err(e) = st.write_all(&wire) {
        return fail(format!("write failed: {e}"));
    }
    let mut raw = Vec::new();
    if let Err(e) = st.read_to_end(&mut raw) {
        if raw.is_empty() {
            return fail(format!("read failed: {e}"));
        }
    }
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return fail(format!("malformed response ({} bytes)", raw.len()));
    };
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("").to_string();
    let mut parts = status_line.splitn(3, ' ');
    let _ver = parts.next();
    let status = parts.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let reason = parts.next().unwrap_or("").to_string();
    let headers = parse_header_lines(&lines.collect::<Vec<_>>().join("\n"));
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.to_lowercase().contains("chunked"));
    let body_raw = &raw[split + 4..];
    let body_bytes = if chunked { dechunk(body_raw) } else { body_raw.to_vec() };
    ComposerResponse {
        status,
        reason,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
        elapsed_ms: t0.elapsed().as_millis(),
        error: if status == 0 { format!("bad status line: {status_line}") } else { String::new() },
    }
}

#[cfg(test)]
mod composer_tests {
    use super::*;

    #[test]
    fn wire_format_has_auto_headers() {
        let r = ComposerRequest { method: "post".into(), url: "http://127.0.0.1:8898/x?a=1".into(), headers: parse_header_lines("X-A: 1\nbad line"), body: "hello".into() };
        let w = String::from_utf8(r.to_wire().unwrap()).unwrap();
        assert!(w.starts_with("POST http://127.0.0.1:8898/x?a=1 HTTP/1.1\r\nHost: 127.0.0.1:8898\r\nX-A: 1\r\n"));
        assert!(w.contains("Content-Length: 5\r\n") && w.ends_with("\r\n\r\nhello"));
        assert!(ComposerRequest { url: "https://a/".into(), ..Default::default() }.to_wire().is_err());
    }

    #[test]
    fn dechunk_works() {
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
    }
}
'''

UI_IMPL = r'''

impl GridApp {
    /// Dispatch the current Composer request through the capture proxy on a worker thread.
    pub fn composer_send(&mut self, ctx: &egui::Context) {
        let req = crate::model::ComposerRequest {
            method: self.comp_method.clone(),
            url: self.comp_url.clone(),
            headers: crate::model::parse_header_lines(&self.comp_headers),
            body: self.comp_body.clone(),
        };
        self.comp_sending = true;
        let tx = self.comp_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = crate::model::composer_send(&req, &format!("127.0.0.1:{}", crate::proxy::PORT));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll_composer(&mut self) {
        while let Ok(r) = self.comp_rx.try_recv() {
            self.comp_sending = false;
            self.comp_resp = Some(r);
        }
    }

    /// Composer window: method, URL, headers, body, Send; response summary below.
    pub fn composer_window(&mut self, ctx: &egui::Context) {
        self.poll_composer();
        if !self.show_composer { return; }
        let mut open = true;
        egui::Window::new("Composer").open(&mut open).default_width(560.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("comp_method").selected_text(self.comp_method.clone()).show_ui(ui, |ui| {
                    for m in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
                        ui.selectable_value(&mut self.comp_method, m.to_string(), m);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut self.comp_url).hint_text("http://127.0.0.1:8898/path").desired_width(340.0));
                let label = if self.comp_sending { "Sending..." } else { "Send" };
                if ui.add_enabled(!self.comp_sending, egui::Button::new(label)).clicked() {
                    self.composer_send(ctx);
                }
            });
            ui.label("Headers (Name: value, one per line)");
            ui.add(egui::TextEdit::multiline(&mut self.comp_headers).desired_rows(3).desired_width(f32::INFINITY));
            ui.label("Body");
            ui.add(egui::TextEdit::multiline(&mut self.comp_body).desired_rows(4).desired_width(f32::INFINITY));
            ui.separator();
            match &self.comp_resp {
                None => { ui.label(RichText::new("(not sent)").color(Color32::GRAY)); }
                Some(r) if !r.error.is_empty() => { ui.label(RichText::new(format!("Error: {}", r.error)).color(Color32::LIGHT_RED)); }
                Some(r) => {
                    ui.label(RichText::new(format!("{} {}  |  {} ms  |  {} headers", r.status, r.reason, r.elapsed_ms, r.headers.len())).strong());
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for (k, v) in &r.headers { ui.label(RichText::new(format!("{k}: {v}")).monospace().small()); }
                        ui.separator();
                        ui.label(RichText::new(&r.body).monospace());
                    });
                }
            }
        });
        self.show_composer = open;
    }
}
'''

def sub1(s, old, new):
    assert s.count(old) == 1, (old, s.count(old))
    return s.replace(old, new)

m = open(M, encoding="utf-8").read()
if "composer_send" not in m:
    open(M + ".pre-composer", "w", encoding="utf-8").write(m)
    open(M, "w", encoding="utf-8").write(m + MODEL_ADD)

u = open(U, encoding="utf-8").read()
if "comp_method" not in u:
    open(U + ".pre-composer", "w", encoding="utf-8").write(u)
    u = sub1(u, "    dump_in: u8,\n}", "    dump_in: u8,\n    pub show_composer: bool,\n    ctx_for_hook: Option<egui::Context>,\n    pub comp_method: String,\n    pub comp_url: String,\n    pub comp_headers: String,\n    pub comp_body: String,\n    pub comp_resp: Option<crate::model::ComposerResponse>,\n    pub comp_sending: bool,\n    comp_tx: std::sync::mpsc::Sender<crate::model::ComposerResponse>,\n    comp_rx: Receiver<crate::model::ComposerResponse>,\n}")
    u = sub1(u, "            https_msg: String::new(),\n        }", "            https_msg: String::new(),\n            show_composer: false,\n            ctx_for_hook: None,\n            comp_method: \"GET\".into(),\n            comp_url: String::new(),\n            comp_headers: String::new(),\n            comp_body: String::new(),\n            comp_resp: None,\n            comp_sending: false,\n            comp_tx,\n            comp_rx,\n        }")
    u = sub1(u, "        let last_saved = serde_json::to_string(&prefs).unwrap_or_default();\n        Self {", "        let last_saved = serde_json::to_string(&prefs).unwrap_or_default();\n        let (comp_tx, comp_rx) = std::sync::mpsc::channel();\n        Self {")
    u = sub1(u, '                if ui.button("HTTPS Settings").clicked() { self.show_https = !self.show_https; }', '                if ui.button("HTTPS Settings").clicked() { self.show_https = !self.show_https; }\n                if ui.selectable_label(self.show_composer, "Composer").clicked() { self.show_composer = !self.show_composer; }')
    u = sub1(u, "        self.https_window(ui.ctx());\n", "        self.https_window(ui.ctx());\n        self.composer_window(ui.ctx());\n")
    u = sub1(u, "                _ => {}\n            }\n        }\n        self.dump_in = 3;",
        '                "composer" => self.show_composer = v == "1",\n                "cmethod" => self.comp_method = v.to_string(),\n                "curl" => self.comp_url = v.to_string(),\n                "cheaders" => self.comp_headers = v.replace(";;", "\\n"),\n                "cbody" => self.comp_body = v.to_string(),\n                "csend" => { if let Some(c) = self.ctx_for_hook.clone() { self.composer_send(&c); } }\n                _ => {}\n            }\n        }\n        self.dump_in = 3;')
    u = sub1(u, '"search": self.model.search_query}});', '"search": self.model.search_query},\n            "composer": {"open": self.show_composer, "sending": self.comp_sending, "method": self.comp_method, "url": self.comp_url, "response": self.comp_resp}});')
    u = sub1(u, "        self.test_hook();\n", "        self.ctx_for_hook = Some(ui.ctx().clone());\n        self.test_hook();\n")
    u += UI_IMPL
    open(U, "w", encoding="utf-8").write(u)
print("patched")

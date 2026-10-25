import sys
p = r"D:\Projects\Ferrotap\src\proxy.rs"
raw = open(p, "rb").read().decode("utf-8")
crlf = "\r\n" in raw
t = raw.replace("\r\n", "\n")
if "breakpoint_hub()" in t:
    print("already patched"); sys.exit(0)

def rep(old, new):
    global t
    assert t.count(old) == 1, ("anchor count", t.count(old), old[:50])
    t = t.replace(old, new)

helpers = '''
const BP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

fn bp_headers(h: &hudsucker::hyper::HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).to_string())).collect()
}

fn bp_set_headers(h: &mut hudsucker::hyper::HeaderMap, list: &[(String, String)]) {
    use hudsucker::hyper::header::{HeaderName, HeaderValue};
    h.clear();
    for (k, v) in list {
        if let (Ok(n), Ok(val)) = (HeaderName::from_bytes(k.trim().as_bytes()), HeaderValue::from_str(v.trim())) {
            h.append(n, val);
        }
    }
}

fn bp_fix_len(h: &mut hudsucker::hyper::HeaderMap, len: usize) {
    h.remove("transfer-encoding");
    h.insert("content-length", hudsucker::hyper::header::HeaderValue::from(len));
}

/// Response the client sees when the user aborts a paused exchange.
fn bp_abort_response() -> Response<Body> {
    let msg = "Aborted at breakpoint by Ferrotap";
    Response::builder()
        .status(502)
        .header("content-type", "text/plain")
        .header("content-length", msg.len().to_string())
        .header("x-ferrotap-breakpoint", "aborted")
        .body(Body::from(Full::new(Bytes::from(msg))))
        .unwrap_or_else(|_| Response::new(Body::from(Full::new(Bytes::new()))))
}

impl HttpHandler for Recorder {'''
rep("\nimpl HttpHandler for Recorder {", helpers)

rep("let (parts, body) = req.into_parts();\n        let bytes: Bytes", "let (mut parts, body) = req.into_parts();\n        let mut bytes: Bytes")
rep("let url = clean_url(&scheme, &authority, pq);", "let mut url = clean_url(&scheme, &authority, pq);")

req_block = '''        // Breakpoints (request stage): park the request until the UI resumes/aborts it.
        if let Some(rule) = crate::model::breakpoint_match(crate::model::BpStage::Request, parts.method.as_str(), &url, 0) {
            let info = crate::model::PausedInfo {
                id: 0,
                stage: crate::model::BpStage::Request,
                rule: rule.clone(),
                method: parts.method.to_string(),
                url: url.clone(),
                status: 0,
                headers: bp_headers(&parts.headers),
                body: bytes.to_vec(),
                since_ms: 0,
            };
            match crate::model::breakpoint_hub().pause(info, BP_TIMEOUT).await {
                crate::model::PauseOutcome::Resumed(e) => {
                    if let Some(m) = e.method {
                        if let Ok(m) = hudsucker::hyper::Method::from_bytes(m.trim().to_ascii_uppercase().as_bytes()) {
                            parts.method = m;
                        }
                    }
                    if let Some(u) = e.url {
                        if let Ok(uri) = u.trim().parse::<hudsucker::hyper::Uri>() {
                            parts.uri = uri;
                            url = u.trim().to_string();
                        }
                    }
                    if let Some(h) = e.headers {
                        bp_set_headers(&mut parts.headers, &h);
                    }
                    if let Some(b) = e.body {
                        bytes = Bytes::from(b);
                        bp_fix_len(&mut parts.headers, bytes.len());
                    }
                }
                crate::model::PauseOutcome::Aborted => {
                    self.pending = None;
                    self.store.push(SessionRecord {
                        id: 0,
                        method: parts.method.to_string(),
                        url: url.clone(),
                        status: 502,
                        http_version: http_version.clone(),
                        response_http_version: "HTTP/1.1".into(),
                        request_headers: hdrs(&parts.headers),
                        request_body: String::from_utf8_lossy(&bytes).to_string(),
                        response_body: "Aborted at breakpoint by Ferrotap".into(),
                        start_time: start_time.clone(),
                        duration_ms: started.elapsed().as_millis(),
                        comment: format!("Breakpoint aborted: {rule}"),
                        ..Default::default()
                    });
                    return bp_abort_response().into();
                }
                crate::model::PauseOutcome::TimedOut => {}
            }
        }
'''
rep("        self.pending = Some(Arc::new(Pending {\n            http_version,", req_block + "        self.pending = Some(Arc::new(Pending {\n            http_version,")

rep("let (parts, body) = res.into_parts();\n        let bytes: Bytes = body.collect().await.map(|c| c.to_bytes()).unwrap_or_default();\n",
'''let (mut parts, body) = res.into_parts();
        let mut bytes: Bytes = body.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
        // Breakpoints (response stage): match on status too.
        if let Some(rule) = crate::model::breakpoint_match(crate::model::BpStage::Response, &p.method, &p.url, parts.status.as_u16()) {
            let info = crate::model::PausedInfo {
                id: 0,
                stage: crate::model::BpStage::Response,
                rule: rule.clone(),
                method: p.method.clone(),
                url: p.url.clone(),
                status: parts.status.as_u16(),
                headers: bp_headers(&parts.headers),
                body: bytes.to_vec(),
                since_ms: 0,
            };
            match crate::model::breakpoint_hub().pause(info, BP_TIMEOUT).await {
                crate::model::PauseOutcome::Resumed(e) => {
                    if let Some(s) = e.status {
                        if let Ok(sc) = hudsucker::hyper::StatusCode::from_u16(s) {
                            parts.status = sc;
                        }
                    }
                    if let Some(h) = e.headers {
                        bp_set_headers(&mut parts.headers, &h);
                    }
                    if let Some(b) = e.body {
                        bytes = Bytes::from(b);
                        bp_fix_len(&mut parts.headers, bytes.len());
                    }
                }
                crate::model::PauseOutcome::Aborted => {
                    self.store.push(SessionRecord {
                        id: 0,
                        method: p.method.clone(),
                        url: p.url.clone(),
                        status: 502,
                        http_version: p.http_version.clone(),
                        response_http_version: "HTTP/1.1".into(),
                        request_headers: p.headers.clone(),
                        request_body: p.body.clone(),
                        response_body: "Aborted at breakpoint by Ferrotap".into(),
                        start_time: p.start_time.clone(),
                        duration_ms: p.started.elapsed().as_millis(),
                        comment: format!("Breakpoint aborted: {rule}"),
                        ..Default::default()
                    });
                    return bp_abort_response();
                }
                crate::model::PauseOutcome::TimedOut => {}
            }
        }
''')
out = t.replace("\n", "\r\n") if crlf else t
open(p, "wb").write(out.encode("utf-8"))
print("patched")

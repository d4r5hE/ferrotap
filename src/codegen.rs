//! Code generation helpers for exporting HTTP sessions (cURL, HTTPie, PowerShell, Fetch, Python
//! requests, C#, Go, Rust).
use crate::model::SessionRecord;

/// Wrap a value in POSIX shell single quotes (safe for bash/sh/git-bash/macOS: no expansion of
/// `$`, backticks or backslashes). A literal `'` becomes the standard `'\''` sequence.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Generates a cURL command representation of the session (bash-style quoting).
pub fn to_curl(s: &SessionRecord) -> String {
    let mut cmd = format!("curl -X {}", s.method);
    for (k, v) in &s.request_headers {
        cmd.push_str(&format!(" -H {}", sh_quote(&format!("{k}: {v}"))));
    }
    if !s.request_body.is_empty() {
        // --data-raw (not --data): send the body verbatim, without stripping newlines or treating a
        // leading '@' as a filename.
        cmd.push_str(&format!(" --data-raw {}", sh_quote(&s.request_body)));
    }
    cmd.push_str(&format!(" {}", sh_quote(&s.url)));
    cmd
}

/// Escape a value for a PowerShell double-quoted string (backtick is the escape char; `$` triggers
/// variable expansion, so both must be escaped, along with the quote).
fn ps_quote(s: &str) -> String {
    s.replace('`', "``").replace('"', "`\"").replace('$', "`$")
}

/// Generates a PowerShell Invoke-RestMethod representation of the session.
pub fn to_powershell(s: &SessionRecord) -> String {
    let ctype = s.request_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone());
    let mut ps = "$headers = @{\n".to_string();
    for (k, v) in &s.request_headers {
        // Invoke-RestMethod rejects Content-Type in -Headers (it needs -ContentType), and manages
        // Content-Length itself; keep both out of the hashtable.
        if k.eq_ignore_ascii_case("content-type") || k.eq_ignore_ascii_case("content-length") {
            continue;
        }
        ps.push_str(&format!("    \"{k}\" = \"{}\"\n", ps_quote(v)));
    }
    ps.push_str("}\n");
    let ct = ctype.map(|c| format!(" -ContentType \"{}\"", ps_quote(&c))).unwrap_or_default();
    let url = ps_quote(&s.url);
    if !s.request_body.is_empty() {
        ps.push_str(&format!("$body = \"{}\"\n", ps_quote(&s.request_body)));
        ps.push_str(&format!("Invoke-RestMethod -Uri \"{url}\" -Method {method} -Headers $headers{ct} -Body $body\n", method = s.method));
    } else {
        ps.push_str(&format!("Invoke-RestMethod -Uri \"{url}\" -Method {method} -Headers $headers{ct}\n", method = s.method));
    }
    ps
}

/// Generates a JavaScript fetch() snippet.
pub fn to_fetch(s: &SessionRecord) -> String {
    let mut headers_obj = String::from("{\n");
    for (k, v) in &s.request_headers {
        headers_obj.push_str(&format!("    \"{k}\": \"{}\",\n", esc_dq(v)));
    }
    headers_obj.push_str("  }");

    let mut opts = format!("  method: \"{method}\",\n  headers: {headers}", method = s.method, headers = headers_obj);
    if !s.request_body.is_empty() && s.method != "GET" && s.method != "HEAD" {
        opts.push_str(&format!(",\n  body: \"{}\"", esc_dq(&s.request_body)));
    }
    format!("fetch(\"{url}\", {{\n{opts}\n}})\n  .then(res => res.text())\n  .then(data => console.log(data));", url = s.url, opts = opts)
}

/// Generates a Python requests snippet.
pub fn to_python(s: &SessionRecord) -> String {
    let mut headers_dict = String::from("headers = {\n");
    for (k, v) in &s.request_headers {
        headers_dict.push_str(&format!("    \"{k}\": \"{}\",\n", esc_dq(v)));
    }
    headers_dict.push_str("}\n");

    let mut code = String::from("import requests\n\n");
    code.push_str(&headers_dict);
    if !s.request_body.is_empty() {
        code.push_str(&format!("data = \"{}\"\n", esc_dq(&s.request_body)));
        code.push_str(&format!("response = requests.{method_lower}(\"{url}\", headers=headers, data=data)\nprint(response.text)",
            method_lower = s.method.to_lowercase(), url = s.url));
    } else {
        code.push_str(&format!("response = requests.{method_lower}(\"{url}\", headers=headers)\nprint(response.text)",
            method_lower = s.method.to_lowercase(), url = s.url));
    }
    code
}

/// Escape the inner text of a double-quoted, C-like string literal (no surrounding quotes). Shared
/// by the C#/Go/Rust/Python/JS generators: a raw newline or CR is illegal inside such a literal.
fn esc_dq(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            _ => o.push(c),
        }
    }
    o
}

/// Escape for a C-like double-quoted string literal, with the surrounding quotes (C#, Go, Rust).
fn q(s: &str) -> String {
    format!("\"{}\"", esc_dq(s))
}

/// Headers an HTTP client sets for itself from the URL and body; emitting the captured value would
/// be redundant or wrong (a stale Content-Length breaks the request). Content-Type is NOT one of
/// these - most clients only send it if you set it.
fn is_auto_header(k: &str) -> bool {
    k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("content-length")
}

/// Headers that belong to the content (set on the body) rather than on the request in .NET.
fn is_content_header(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k == "content-type" || k == "content-length" || k == "content-encoding" || k == "content-language"
}

/// Generates a C# HttpClient snippet.
pub fn to_csharp(s: &SessionRecord) -> String {
    let mut c = String::from("using System.Net.Http;\nusing System.Text;\n\nvar client = new HttpClient();\n");
    c.push_str(&format!("var request = new HttpRequestMessage(new HttpMethod({}), {});\n", q(&s.method), q(&s.url)));
    let ctype = s.request_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone());
    for (k, v) in &s.request_headers {
        if is_content_header(k) || k.eq_ignore_ascii_case("host") { continue; }
        c.push_str(&format!("request.Headers.TryAddWithoutValidation({}, {});\n", q(k), q(v)));
    }
    if !s.request_body.is_empty() {
        let mt = ctype.as_deref().unwrap_or("text/plain");
        let mt = mt.split(';').next().unwrap_or(mt).trim();
        c.push_str(&format!("request.Content = new StringContent({}, Encoding.UTF8, {});\n", q(&s.request_body), q(mt)));
    }
    c.push_str("var response = await client.SendAsync(request);\nConsole.WriteLine((int)response.StatusCode);\nConsole.WriteLine(await response.Content.ReadAsStringAsync());\n");
    c
}

/// Generates a Go net/http snippet.
pub fn to_go(s: &SessionRecord) -> String {
    let mut c = String::from("package main\n\nimport (\n\t\"fmt\"\n\t\"io\"\n\t\"net/http\"\n\t\"strings\"\n)\n\nfunc main() {\n");
    let body = if s.request_body.is_empty() { "nil".to_string() } else { format!("strings.NewReader({})", q(&s.request_body)) };
    c.push_str(&format!("\treq, err := http.NewRequest({}, {}, {})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n", q(&s.method), q(&s.url), body));
    for (k, v) in &s.request_headers {
        if is_auto_header(k) { continue; }
        c.push_str(&format!("\treq.Header.Set({}, {})\n", q(k), q(v)));
    }
    c.push_str("\tres, err := http.DefaultClient.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer res.Body.Close()\n\tdata, _ := io.ReadAll(res.Body)\n\tfmt.Println(res.Status)\n\tfmt.Println(string(data))\n}\n");
    if s.request_body.is_empty() { c = c.replace("\t\"strings\"\n", ""); }
    c
}

/// Generates a Rust reqwest (blocking) snippet.
pub fn to_rust(s: &SessionRecord) -> String {
    let mut c = String::from("// Cargo.toml: reqwest = { version = \"0.12\", features = [\"blocking\"] }\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let client = reqwest::blocking::Client::new();\n");
    c.push_str(&format!("    let res = client\n        .request(reqwest::Method::from_bytes({}.as_bytes())?, {})\n", q(&s.method), q(&s.url)));
    for (k, v) in &s.request_headers {
        if is_auto_header(k) { continue; }
        c.push_str(&format!("        .header({}, {})\n", q(k), q(v)));
    }
    if !s.request_body.is_empty() {
        c.push_str(&format!("        .body({})\n", q(&s.request_body)));
    }
    c.push_str("        .send()?;\n    println!(\"{}\", res.status());\n    println!(\"{}\", res.text()?);\n    Ok(())\n}\n");
    c
}

/// Generates a wget command line.
pub fn to_wget(s: &SessionRecord) -> String {
    let mut cmd = format!("wget --method={}", s.method);
    for (k, v) in &s.request_headers {
        // wget derives Host from the URL and sets Content-Length for --body-data; a captured
        // value would be redundant or (for a stale Content-Length) wrong.
        if is_auto_header(k) {
            continue;
        }
        cmd.push_str(&format!(" --header={}", sh_quote(&format!("{k}: {v}"))));
    }
    if !s.request_body.is_empty() {
        cmd.push_str(&format!(" --body-data={}", sh_quote(&s.request_body)));
    }
    // -qO- : quiet, write the response body to stdout.
    cmd.push_str(&format!(" -qO- {}", sh_quote(&s.url)));
    cmd
}

/// Generates an HTTPie command line (https://httpie.io).
pub fn to_httpie(s: &SessionRecord) -> String {
    let mut cmd = format!("http {} {}", s.method, sh_quote(&s.url));
    for (k, v) in &s.request_headers {
        // HTTPie derives Host from the URL and computes Content-Length itself; a captured
        // value would be redundant or (for a stale Content-Length) wrong. Content-Type stays:
        // with --raw, HTTPie does not set it for you.
        if is_auto_header(k) {
            continue;
        }
        // `Header:Value` is HTTPie's header syntax; quoting the whole token keeps a value with
        // spaces or shell metacharacters intact, and avoids it being read as a `==`/`=` item.
        cmd.push_str(&format!(" {}", sh_quote(&format!("{k}:{v}"))));
    }
    if !s.request_body.is_empty() {
        // --raw sends the body verbatim, instead of HTTPie's key=value / JSON-field parsing.
        cmd.push_str(&format!(" --raw={}", sh_quote(&s.request_body)));
    }
    cmd
}

/// Languages offered by the Composer "Code" view, in display order.
pub const LANGUAGES: [&str; 9] = ["cURL", "HTTPie", "wget", "PowerShell", "Python", "JavaScript fetch", "C#", "Go", "Rust"];

/// Generate code for `lang` (one of `LANGUAGES`).
pub fn generate(lang: &str, s: &SessionRecord) -> String {
    match lang {
        "cURL" => to_curl(s),
        "HTTPie" => to_httpie(s),
        "wget" => to_wget(s),
        "PowerShell" => to_powershell(s),
        "Python" => to_python(s),
        "JavaScript fetch" => to_fetch(s),
        "C#" => to_csharp(s),
        "Go" => to_go(s),
        "Rust" => to_rust(s),
        _ => format!("unknown language {lang}"),
    }
}

/// Split a shell-ish command line into tokens, honouring single quotes (literal), double quotes
/// (with `\"`, `\\`, `\$`, `` \` `` escapes) and `\`-newline line continuations. Good enough for
/// the cURL commands people copy from browser devtools.
fn shell_tokens(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        // Skip separators and line continuations.
        while i < c.len() && (c[i].is_whitespace() || (c[i] == '\\' && i + 1 < c.len() && (c[i + 1] == '\n' || c[i + 1] == '\r'))) {
            if c[i] == '\\' {
                i += 1;
            }
            i += 1;
        }
        if i >= c.len() {
            break;
        }
        let mut tok = String::new();
        let mut any = false;
        while i < c.len() && !c[i].is_whitespace() {
            match c[i] {
                '\'' => {
                    any = true;
                    i += 1;
                    while i < c.len() && c[i] != '\'' {
                        tok.push(c[i]);
                        i += 1;
                    }
                    i += 1; // closing quote (or end)
                }
                '"' => {
                    any = true;
                    i += 1;
                    while i < c.len() && c[i] != '"' {
                        if c[i] == '\\' && i + 1 < c.len() && matches!(c[i + 1], '"' | '\\' | '$' | '`') {
                            tok.push(c[i + 1]);
                            i += 2;
                        } else {
                            tok.push(c[i]);
                            i += 1;
                        }
                    }
                    i += 1;
                }
                '\\' if i + 1 < c.len() && (c[i + 1] == '\n' || c[i + 1] == '\r') => break,
                '\\' if i + 1 < c.len() => {
                    tok.push(c[i + 1]);
                    i += 2;
                }
                ch => {
                    tok.push(ch);
                    i += 1;
                }
            }
        }
        if any || !tok.is_empty() {
            out.push(tok);
        }
    }
    out
}

/// Unary cURL flags that take no argument (so their presence must not swallow the next token).
fn is_curl_bareflag(f: &str) -> bool {
    matches!(
        f,
        "--compressed" | "-s" | "--silent" | "-S" | "--show-error" | "-k" | "--insecure" | "-L" | "--location"
            | "-v" | "--verbose" | "-i" | "--include" | "-g" | "--globoff" | "-f" | "--fail" | "-#" | "--progress-bar"
            | "-O" | "--remote-name" | "-j" | "--junk-session-cookies" | "--http1.1" | "--http2" | "--http2-prior-knowledge"
    )
}

/// cURL flags that take a value we don't model (skip the value too).
fn is_curl_skip_with_value(f: &str) -> bool {
    matches!(
        f,
        "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-x" | "--proxy" | "-w" | "--write-out"
            | "-c" | "--cookie-jar" | "-K" | "--config" | "-E" | "--cert" | "--key" | "--cacert" | "--retry" | "-O"
    )
}

/// Advance `i` and return the following token (the value after a flag).
fn curl_next(toks: &[String], i: &mut usize) -> Option<String> {
    *i += 1;
    toks.get(*i).cloned()
}

/// Parse a cURL command line into a Composer request. Returns `None` if no URL is found.
/// Recognises -X/--request, -H/--header, -d/--data[-raw|-binary|-ascii], --url, -b/--cookie,
/// -A/--user-agent, -e/--referer and -u/--user; other flags are ignored.
pub fn parse_curl(cmd: &str) -> Option<crate::model::ComposerRequest> {
    let toks = shell_tokens(cmd);
    let mut method: Option<String> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut get_mode = false; // -G / --get moves the data onto the query string
    let add_header = |headers: &mut Vec<(String, String)>, k: &str, v: &str| {
        headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
        headers.push((k.to_string(), v.to_string()));
    };
    let mut i = 0;
    // Skip a leading "curl".
    if toks.first().is_some_and(|t| t.eq_ignore_ascii_case("curl")) {
        i = 1;
    }
    while i < toks.len() {
        let t = toks[i].clone();
        match t.as_str() {
            "-X" | "--request" => {
                if let Some(m) = curl_next(&toks, &mut i) {
                    method = Some(m.to_uppercase());
                }
            }
            _ if t.starts_with("-X") && t.len() > 2 => method = Some(t[2..].to_uppercase()),
            "-H" | "--header" => {
                if let Some(h) = curl_next(&toks, &mut i) {
                    if let Some((k, v)) = h.split_once(':') {
                        add_header(&mut headers, k.trim(), v.trim());
                    }
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                if let Some(d) = curl_next(&toks, &mut i) {
                    data.push(d);
                }
            }
            // curl 7.82+: --json sets the body and the JSON content-type/accept headers.
            "--json" => {
                if let Some(d) = curl_next(&toks, &mut i) {
                    data.push(d);
                    add_header(&mut headers, "Content-Type", "application/json");
                    add_header(&mut headers, "Accept", "application/json");
                }
            }
            "--data-urlencode" => {
                if let Some(d) = curl_next(&toks, &mut i) {
                    // Forms: content | =content | name=content (percent-encode the value part).
                    // @file forms can't be read here, so keep them verbatim.
                    if let Some(rest) = d.strip_prefix('=') {
                        data.push(crate::model::pct_encode(rest));
                    } else if d.contains('@') {
                        data.push(d);
                    } else if let Some((name, val)) = d.split_once('=') {
                        data.push(format!("{}={}", name, crate::model::pct_encode(val)));
                    } else {
                        data.push(crate::model::pct_encode(&d));
                    }
                }
            }
            "-G" | "--get" => get_mode = true,
            "--url" => url = curl_next(&toks, &mut i),
            "-b" | "--cookie" => {
                if let Some(ck) = curl_next(&toks, &mut i) {
                    if ck.contains('=') {
                        add_header(&mut headers, "Cookie", &ck);
                    }
                }
            }
            "-A" | "--user-agent" => {
                if let Some(ua) = curl_next(&toks, &mut i) {
                    add_header(&mut headers, "User-Agent", &ua);
                }
            }
            "-e" | "--referer" => {
                if let Some(r) = curl_next(&toks, &mut i) {
                    add_header(&mut headers, "Referer", &r);
                }
            }
            "-u" | "--user" => {
                if let Some(up) = curl_next(&toks, &mut i) {
                    use base64::Engine as _;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(up.as_bytes());
                    add_header(&mut headers, "Authorization", &format!("Basic {b64}"));
                }
            }
            _ if is_curl_skip_with_value(&t) => {
                curl_next(&toks, &mut i);
            }
            _ if is_curl_bareflag(&t) => {}
            _ if t.starts_with('-') => {} // unknown flag: ignore, don't swallow a value
            _ => {
                // A bare token: the URL (prefer one with a scheme).
                if url.is_none() || t.contains("://") {
                    url = Some(t.clone());
                }
            }
        }
        i += 1;
    }
    let mut url = url?;
    let body = data.join("&");
    // -G: the collected data becomes the query string and the request stays bodyless.
    if get_mode && !body.is_empty() {
        let sep = if url.contains('?') { '&' } else { '?' };
        url.push(sep);
        url.push_str(&body);
        let method = method.unwrap_or_else(|| "GET".into());
        return Some(crate::model::ComposerRequest { method, url, headers, body: String::new() });
    }
    let method = method.unwrap_or_else(|| if body.is_empty() { "GET".into() } else { "POST".into() });
    Some(crate::model::ComposerRequest { method, url, headers, body })
}

/// A SessionRecord holding just the request part of a Composer request (for the generators).
pub fn session_from_request(r: &crate::model::ComposerRequest) -> SessionRecord {
    let mut s = SessionRecord::default();
    s.method = if r.method.trim().is_empty() { "GET".into() } else { r.method.trim().to_uppercase() };
    s.url = r.url.trim().to_string();
    for (k, v) in &r.headers { s.request_headers.insert(k.clone(), v.clone()); }
    s.request_body = r.body.clone();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sample_session() -> SessionRecord {
        let mut s = SessionRecord::default();
        s.id = 1;
        s.method = "POST".into();
        s.url = "https://httpbin.org/post".into();
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("Authorization".into(), "Bearer token123".into());
        s.request_headers = headers;
        s.request_body = "{\"hello\": \"world\"}".into();
        s
    }

    #[test]
    fn test_code_generators() {
        let s = sample_session();
        let curl = to_curl(&s);
        assert!(curl.contains("curl -X POST"));
        assert!(curl.contains("Bearer token123"));
        assert!(curl.contains("https://httpbin.org/post"));

        let ps = to_powershell(&s);
        assert!(ps.contains("Invoke-RestMethod"));
        assert!(ps.contains("Bearer token123"));

        let fetch = to_fetch(&s);
        assert!(fetch.contains("fetch(\"https://httpbin.org/post\""));
        assert!(fetch.contains("method: \"POST\""));

        let py = to_python(&s);
        assert!(py.contains("requests.post"));
        assert!(py.contains("Bearer token123"));

        let wget = to_wget(&s);
        assert!(wget.starts_with("wget --method=POST"), "{wget}");
        assert!(wget.contains("--header='Authorization: Bearer token123'"), "{wget}");
        assert!(wget.contains(r#"--body-data='{"hello": "world"}'"#), "{wget}");
        assert!(wget.ends_with("-qO- 'https://httpbin.org/post'"), "{wget}");

        let httpie = to_httpie(&s);
        assert!(httpie.starts_with("http POST 'https://httpbin.org/post'"), "{httpie}");
        assert!(httpie.contains("'Authorization:Bearer token123'"), "{httpie}");
        assert!(httpie.contains("'Content-Type:application/json'"), "{httpie}");
        assert!(httpie.contains(r#"--raw='{"hello": "world"}'"#), "{httpie}");
        // Every advertised language must actually generate (no "unknown language" fallthrough).
        for lang in LANGUAGES {
            assert!(!generate(lang, &s).starts_with("unknown language"), "{lang}");
        }
    }

    #[test]
    fn parse_curl_cases() {
        let g = parse_curl("curl https://api.test/a").unwrap();
        assert_eq!((g.method.as_str(), g.url.as_str()), ("GET", "https://api.test/a"));
        assert!(g.headers.is_empty() && g.body.is_empty());

        let c = r#"curl -X POST 'https://api.test/login' -H 'Content-Type: application/json' -H "Accept: */*" --data-raw '{"u":"a","p":"b"}' --compressed"#;
        let g = parse_curl(c).unwrap();
        assert_eq!(g.method, "POST");
        assert_eq!(g.url, "https://api.test/login");
        assert_eq!(g.body, r#"{"u":"a","p":"b"}"#);
        assert!(g.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
        assert!(g.headers.iter().any(|(k, v)| k == "Accept" && v == "*/*"));

        // -d implies POST; backslash line-continuation; multiple -d join with &.
        let g = parse_curl("curl https://h/x \\\n  -d 'a=1' \\\n  -d 'b=2'").unwrap();
        assert_eq!((g.method.as_str(), g.body.as_str()), ("POST", "a=1&b=2"));

        // Attached -X form, and a value-taking flag (-o) must not be mistaken for the URL.
        assert_eq!(parse_curl("curl -XPUT https://h/y").unwrap().method, "PUT");
        let g = parse_curl("curl -u me:secret -o out.txt https://h/z").unwrap();
        assert_eq!(g.url, "https://h/z");
        assert!(g.headers.iter().any(|(k, v)| k == "Authorization" && v.starts_with("Basic ")));

        assert!(parse_curl("curl -X GET").is_none()); // no URL -> None

        // --json sets the body and the JSON headers, implies POST.
        let g = parse_curl(r#"curl --json '{"a":1}' https://h/j"#).unwrap();
        assert_eq!((g.method.as_str(), g.body.as_str()), ("POST", r#"{"a":1}"#));
        assert!(g.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
        assert!(g.headers.iter().any(|(k, v)| k == "Accept" && v == "application/json"));

        // --data-urlencode percent-encodes the value part.
        let g = parse_curl("curl --data-urlencode 'q=a b&c' https://h/s").unwrap();
        assert_eq!(g.body, "q=a%20b%26c");

        // -G moves the data to the query string and keeps the request bodyless.
        let g = parse_curl("curl -G -d q=hello -d n=2 https://h/search").unwrap();
        assert_eq!((g.method.as_str(), g.url.as_str(), g.body.as_str()), ("GET", "https://h/search?q=hello&n=2", ""));
    }

    #[test]
    fn httpie_skips_auto_headers_and_quotes_safely() {
        let mut s = SessionRecord::default();
        s.method = "GET".into();
        s.url = "https://ex.test/a b".into(); // a space to prove the URL is quoted
        s.request_headers.insert("Host".into(), "ex.test".into());
        s.request_headers.insert("Content-Length".into(), "999".into());
        s.request_headers.insert("X-Note".into(), "a'b$c".into());
        let h = to_httpie(&s);
        assert!(!h.contains("Host:"), "{h}"); // Host is derived from the URL
        assert!(!h.contains("Content-Length"), "{h}"); // computed by HTTPie
        assert!(h.contains("'https://ex.test/a b'"), "{h}");
        // Single-quoting leaves $ and backticks literal; an embedded ' becomes '\'' .
        assert!(h.contains(r#"'X-Note:a'\''b$c'"#), "{h}");
        assert!(!h.contains("--raw"), "{h}"); // no body on a GET
    }
}

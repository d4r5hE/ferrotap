//! Import Fiddler Classic AutoResponder rule files (.farx). Supported match forms: plain text
//! (contains), `regex:`, `EXACT:`, `NOT:` and an optional `METHOD:VERB ` prefix. Actions: `*NNN`
//! (status), `*drop` / `*reset` (close), `*delay:ms`, `*redir:URL`, `http(s)://...` (Map Remote),
//! a local file path (Map Local). Other `*` actions are imported disabled with a note.
use crate::model::Rule;

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let i = tag.find(&key)? + key.len();
    let v = &tag[i..i + tag[i..].find('"')?];
    Some(v.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&apos;", "'").replace("&amp;", "&"))
}

/// Rules from the text of a .farx file.
pub fn parse_farx(text: &str) -> Result<Vec<Rule>, String> {
    if !text.contains("<AutoResponder") {
        return Err("not a Fiddler AutoResponder (.farx) file".into());
    }
    let mut out = Vec::new();
    for (i, chunk) in text.split("<ResponseRule").skip(1).enumerate() {
        let tag = &chunk[..chunk.find('>').unwrap_or(chunk.len())];
        let Some(m) = attr(tag, "Match") else { continue };
        let action = attr(tag, "Action").unwrap_or_default();
        let enabled = attr(tag, "Enabled").is_none_or(|e| e.eq_ignore_ascii_case("true"));
        let mut r = Rule { name: format!("Fiddler rule {}", i + 1), enabled, ..Default::default() };
        let mut m = m.trim().to_string();
        if let Some(rest) = m.strip_prefix("METHOD:") {
            let (verb, tail) = rest.split_once(' ').unwrap_or((rest, ""));
            r.method = verb.to_ascii_uppercase();
            m = tail.trim().to_string();
        }
        if let Some(re) = m.strip_prefix("regex:") {
            r.match_kind = "regex".into();
            r.pattern = re.to_string();
        } else if let Some(x) = m.strip_prefix("EXACT:") {
            r.match_kind = "regex".into();
            r.pattern = format!("^{}$", regex::escape(x));
        } else if let Some(x) = m.strip_prefix("NOT:") {
            // Rules here have no "URL does not contain": keep the text, import it switched off.
            r.match_kind = "contains".into();
            r.pattern = x.to_string();
            r.enabled = false;
            r.name = format!("{} (NOT: match imported disabled)", r.name);
        } else if m == "*" {
            r.match_kind = "regex".into();
            r.pattern = ".*".into();
        } else {
            r.match_kind = "contains".into();
            r.pattern = m.clone();
        }
        let a = action.trim();
        let lower = a.to_ascii_lowercase();
        if let Some(code) = lower.strip_prefix('*').and_then(|c| c.parse::<u16>().ok()) {
            r.action = "canned".into();
            r.status = code;
        } else if lower == "*drop" || lower == "*reset" || lower == "*exit" {
            r.action = "close".into();
        } else if let Some(ms) = lower.strip_prefix("*delay:") {
            r.action = "modify".into();
            r.delay_ms = ms.trim().parse().unwrap_or(0);
        } else if lower.starts_with("*redir:") {
            r.action = "redirect".into();
            r.status = 302;
            r.target = a[7..].trim().to_string();
        } else if lower.starts_with("http://") || lower.starts_with("https://") {
            r.action = "map_remote".into();
            r.target = a.to_string();
        } else if !a.is_empty() && !a.starts_with('*') {
            r.action = "file".into();
            r.file = a.to_string();
        } else {
            r.action = "modify".into();
            r.enabled = false;
            r.name = format!("{} (unsupported action {a})", r.name);
        }
        r.name = format!("{} {}", r.name, r.pattern.chars().take(40).collect::<String>()).trim().to_string();
        out.push(r);
    }
    for r in out.iter_mut() {
        r.id = crate::rules_ext::new_rule_id();
    }
    Ok(out)
}

//! Do-not-decrypt (HTTPS bypass) domain list. Hosts matching a pattern are tunnelled as raw TLS (no MITM).
//! Patterns: `example.com` (exact host), `*.example.com` (any subdomain), or any glob using `*`.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone)]
pub struct BypassList {
    inner: Arc<RwLock<Vec<String>>>,
    file: PathBuf,
    pub tunneled: Arc<AtomicU64>,
}

/// Normalise user input: strip scheme/path/port, lowercase, trim.
pub fn normalize(p: &str) -> String {
    let mut s = p.trim().to_lowercase();
    if let Some(i) = s.find("://") {
        s = s[i + 3..].to_string();
    }
    if let Some(i) = s.find('/') {
        s.truncate(i);
    }
    if let Some(i) = s.rfind(':') {
        if !s.contains(']') && s[i + 1..].chars().all(|c| c.is_ascii_digit()) {
            s.truncate(i);
        }
    }
    s.trim_matches(|c| c == '[' || c == ']' || c == '.').to_string()
}

/// Host part of a CONNECT authority (`host:443`, `[::1]:443`).
pub fn host_of(authority: &str) -> String {
    normalize(authority)
}

fn glob(pat: &str, text: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == text;
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            return text.len() >= pos + part.len() && text.ends_with(part);
        } else {
            match text[pos..].find(part) {
                Some(f) => pos += f + part.len(),
                None => return false,
            }
        }
    }
    true
}

impl BypassList {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let list: Vec<String> = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { inner: Arc::new(RwLock::new(list)), file, tunneled: Arc::new(AtomicU64::new(0)) }
    }

    pub fn patterns(&self) -> Vec<String> {
        self.inner.read().unwrap().clone()
    }

    /// Replace the whole list (normalised, de-duplicated, blanks dropped) and persist it.
    pub fn set(&self, items: Vec<String>) -> Result<(), String> {
        let mut out: Vec<String> = Vec::new();
        for i in items {
            let n = normalize(&i);
            if !n.is_empty() && !out.contains(&n) {
                out.push(n);
            }
        }
        *self.inner.write().unwrap() = out.clone();
        if let Some(p) = self.file.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let j = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
        std::fs::write(&self.file, j).map_err(|e| e.to_string())
    }

    pub fn add(&self, item: &str) -> Result<(), String> {
        let mut v = self.patterns();
        v.push(item.to_string());
        self.set(v)
    }

    pub fn remove(&self, item: &str) -> Result<(), String> {
        let v = self.patterns().into_iter().filter(|p| p != item).collect();
        self.set(v)
    }

    /// True when `host` must NOT be decrypted.
    pub fn matches(&self, host: &str) -> bool {
        let h = host_of(host);
        if h.is_empty() {
            return false;
        }
        self.inner.read().unwrap().iter().any(|p| {
            if let Some(rest) = p.strip_prefix("*.") {
                if !rest.contains('*') {
                    return h.ends_with(&format!(".{rest}"));
                }
            }
            glob(p, &h)
        })
    }

    pub fn note_tunnel(&self) {
        self.tunneled.fetch_add(1, Ordering::Relaxed);
    }
}

/// Process-wide list shared by the proxy handler and the UI (persisted in `data/bypass.json`).
pub fn global() -> BypassList {
    static G: OnceLock<BypassList> = OnceLock::new();
    G.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        BypassList::new(root.join("data").join("bypass.json"))
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        let dir = std::env::temp_dir().join(format!("bp_{}", std::process::id()));
        let b = BypassList::new(dir.join("b.json"));
        b.set(vec![" https://Bank.com:443/x ".into(), "*.pay.example".into(), "api*.corp.io".into(), "bank.com".into()]).unwrap();
        assert_eq!(b.patterns().len(), 3);
        assert!(b.matches("bank.com:443"));
        assert!(!b.matches("www.bank.com"));
        assert!(b.matches("a.pay.example"));
        assert!(!b.matches("pay.example"));
        assert!(b.matches("api2.corp.io"));
        assert!(!b.matches("evilbank.com"));
        let b2 = BypassList::new(dir.join("b.json"));
        assert_eq!(b2.patterns(), b.patterns());
        b2.remove("bank.com").unwrap();
        assert!(!b2.matches("bank.com"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

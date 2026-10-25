// G11: LLM response cache - key normalization, hit / miss, persistence and the proxy hook.
// The process-wide cache file is redirected to a temp dir (FERROTAP_LLM_CACHE) before first use.
use ferrotap::llm_analysis::*;
use ferrotap::model::SessionRecord;
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn temp_cache_env() {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let d = DIR.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("FERROTAP_LLM_CACHE", d.path().join("llm_cache.json"));
}

const URL: &str = "https://api.openai.com/v1/chat/completions";
const REQ: &str = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Capital of France?"}],"temperature":0}"#;
const RESP: &str = r#"{"model":"gpt-4o-mini","choices":[{"message":{"role":"assistant","content":"Paris"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":1}}"#;

fn captured(id: usize) -> SessionRecord {
    SessionRecord {
        id,
        method: "POST".into(),
        url: URL.into(),
        status: 200,
        response_headers: BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), "99".to_string()),
        ]),
        request_body: REQ.into(),
        response_body: RESP.into(),
        ..Default::default()
    }
}

#[test]
fn key_ignores_json_key_order_and_whitespace() {
    let reordered = r#"{ "temperature": 0,
        "messages": [ { "content": "Capital of France?", "role": "user" } ],
        "model": "gpt-4o-mini" }"#;
    assert_eq!(cache_key("POST", URL, REQ), cache_key("post", URL, reordered));
    assert_ne!(cache_key("POST", URL, REQ), cache_key("POST", URL, &REQ.replace("France", "Spain")));
    assert_ne!(cache_key("POST", URL, REQ), cache_key("POST", "https://openrouter.ai/api/v1/chat/completions", REQ));
    assert_eq!(normalize_body(reordered), r#"{"messages":[{"content":"Capital of France?","role":"user"}],"model":"gpt-4o-mini","temperature":0}"#);
}

#[test]
fn add_lookup_hit_miss_and_persistence() {
    let mut c = LlmCache::default();
    assert!(c.add(&captured(1)));
    assert!(!c.add(&captured(2)), "identical request is cached once");
    let mut failed = captured(3);
    failed.status = 500;
    failed.request_body = REQ.replace("France", "Peru");
    assert!(!c.add(&failed), "errors are not cached");
    let mut not_llm = captured(4);
    not_llm.url = "https://shop.example.com/cart".into();
    assert!(!c.add(&not_llm));
    assert_eq!(c.entries.len(), 1);
    assert_eq!(c.entries[0].model, "gpt-4o-mini");

    let reordered = r#"{"temperature":0,"model":"gpt-4o-mini","messages":[{"content":"Capital of France?","role":"user"}]}"#;
    assert_eq!(c.lookup("POST", URL, reordered).unwrap().response_body, RESP);
    assert!(c.lookup("POST", URL, &REQ.replace("France", "Italy")).is_none());
    assert!(c.lookup("GET", "https://example.com/", "").is_none(), "non-LLM traffic is not counted");
    assert_eq!((c.hits, c.misses, c.entries[0].hits), (1, 1, 1));

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("llm_cache.json");
    c.enabled = true;
    c.save(&p).unwrap();
    let back = LlmCache::load(&p);
    assert!(back.enabled);
    assert_eq!(back.entries, c.entries);
    assert_eq!(back.hits, 0, "run counters are not persisted");
    let key = c.entries[0].key.clone();
    c.remove(&key);
    assert!(c.entries.is_empty());
}

#[test]
fn proxy_hook_serves_hits_only_when_enabled() {
    temp_cache_env();
    {
        let mut g = global_cache().write().unwrap();
        g.entries.clear();
        g.enabled = false;
        assert!(g.add(&captured(7)));
    }
    let h = BTreeMap::new();
    assert!(cache_hook("POST", URL, &h, REQ, "HTTP/1.1", "t").is_none(), "cache off: never served");
    global_cache().write().unwrap().enabled = true;
    let hit = cache_hook("POST", URL, &h, REQ, "HTTP/2.0", "2026-10-16T12:00:00Z").expect("hit");
    assert_eq!(hit.status, 200);
    assert_eq!(hit.body, RESP);
    let names: Vec<String> = hit.headers.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
    assert!(!names.contains(&"content-encoding".to_string()), "the stored body is decoded");
    assert!(hit.headers.contains(&("content-length".to_string(), RESP.len().to_string())));
    assert!(hit.headers.contains(&("x-ferrotap-llm-cache".to_string(), "hit".to_string())));
    assert_eq!(hit.record.response_body, RESP);
    assert!(hit.record.comment.contains("#7"));
    // a served response is itself never re-added to the cache
    let mut g = global_cache().write().unwrap();
    assert!(!g.add(&SessionRecord { request_body: REQ.replace("France", "Chile"), ..hit.record.clone() }));
    drop(g);
    // miss
    assert!(cache_hook("POST", URL, &h, &REQ.replace("France", "Japan"), "HTTP/1.1", "t").is_none());
    let (resp, rec) = proxy_cache_response("POST", URL, &h, REQ.as_bytes(), "HTTP/1.1", "t").expect("proxy response");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-ferrotap-llm-cache").unwrap(), "hit");
    assert_eq!(rec.url, URL);
    global_cache().write().unwrap().enabled = false;
}

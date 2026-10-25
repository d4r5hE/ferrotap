// G11: the AI windows render headlessly over captured sessions (every window open), without the real app or its data.
use ferrotap::llm_analysis::{global_cache, LlmCache, PriceTable};
use ferrotap::llm_traffic::{draw_windows, fmt_cost, fmt_tokens, State};
use ferrotap::model::SessionRecord;
use std::collections::BTreeMap;

fn sess(id: usize, url: &str, req: &str, resp: &str) -> SessionRecord {
    SessionRecord {
        id,
        method: "POST".into(),
        url: url.into(),
        status: 200,
        response_headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        request_body: req.into(),
        response_body: resp.into(),
        start_time: "2026-10-16T12:00:01.000+00:00".into(),
        duration_ms: 900,
        ttfb_ms: Some(300),
        ..Default::default()
    }
}

fn rows() -> Vec<SessionRecord> {
    let u = r#"{"role":"user","content":"list files"}"#;
    let a = r#"{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"ls","arguments":"{}"}}]}"#;
    let t = r#"{"role":"tool","tool_call_id":"c1","content":"a.rs"}"#;
    let tool_reply = r#"{"model":"gpt-4o","choices":[{"message":{"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"ls","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":5}}"#;
    let text_reply = r#"{"model":"gpt-4o","choices":[{"message":{"role":"assistant","content":"One file: a.rs"},"finish_reason":"stop"}],"usage":{"prompt_tokens":70,"completion_tokens":6}}"#;
    let url = "https://api.openai.com/v1/chat/completions";
    vec![
        SessionRecord { id: 1, method: "GET".into(), url: "https://example.com/".into(), status: 200, ..Default::default() },
        sess(2, url, &format!(r#"{{"model":"gpt-4o","messages":[{u}],"tools":[{{"type":"function","function":{{"name":"ls","description":"List"}}}}]}}"#), tool_reply),
        sess(3, url, &format!(r#"{{"model":"gpt-4o","messages":[{u},{a},{t}]}}"#), text_reply),
        sess(4, "https://api.anthropic.com/v1/messages", r#"{"model":"unpriced-model","max_tokens":5,"messages":[{"role":"user","content":"x"}]}"#, r#"{"type":"error","error":{"message":"bad key"}}"#),
        sess(5, "http://localhost:11434/api/chat", r#"{"model":"llama3.2","messages":[{"role":"user","content":"hi"}],"stream":false}"#, r#"{"model":"llama3.2","message":{"role":"assistant","content":"hello"},"done":true,"prompt_eval_count":3,"eval_count":2}"#),
    ]
}

fn frame(ctx: &egui::Context, st: &mut State, rows: &[SessionRecord], sel: Option<usize>, ids: &[usize]) -> Option<usize> {
    let mut picked = None;
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        picked = draw_windows(ui.ctx(), st, rows, sel, ids);
    });
    out.drop_without_applying_deltas();
    picked
}

fn isolated_state() -> (State, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_LLM_CACHE", d.path().join("llm_cache.json"));
    let mut st = State::default();
    st.prices_path = Some(d.path().join("llm_prices.json"));
    (st, d)
}

#[test]
fn all_windows_render_for_every_selection() {
    let (mut st, _d) = isolated_state();
    let v = rows();
    let ctx = egui::Context::default();
    assert!(!st.any_open());
    st.show_inspector = true;
    st.show_calls = true;
    st.show_agents = true;
    st.show_cache = true;
    st.show_prices = true;
    st.show_single_calls = true;
    for sel in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(99)] {
        for _ in 0..2 {
            assert_eq!(frame(&ctx, &mut st, &v, sel, &sel.into_iter().collect::<Vec<_>>()), None);
        }
    }
    assert!(st.any_open() && st.show_inspector && st.show_prices);
    frame(&ctx, &mut st, &[], None, &[]);
    // the price table came from the (missing) temp file: defaults, nothing written
    assert_eq!(st.prices.clone().unwrap(), PriceTable::default());
    assert!(!st.prices_path().exists());
}

#[test]
fn calls_are_parsed_once_and_reparsed_when_a_row_changes() {
    let (mut st, _d) = isolated_state();
    let mut v = rows();
    let calls = st.calls(&v);
    assert_eq!(calls.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2, 3, 4, 5]);
    assert_eq!(calls[1].reply_text(), "One file: a.rs");
    let again = st.calls(&v);
    assert!(std::sync::Arc::ptr_eq(&calls[0], &again[0]), "cached parse reused");
    v[1].response_body = r#"{"model":"gpt-4o","choices":[{"message":{"role":"assistant","content":"changed!"},"finish_reason":"stop"}]}"#.into();
    assert_eq!(st.calls(&v)[0].reply_text(), "changed!");
}

#[test]
fn auto_add_fills_the_cache_from_new_rows() {
    let (mut st, d) = isolated_state();
    let _ = LlmCache::default_path();
    {
        let mut g = global_cache().write().unwrap();
        g.entries.clear();
        g.auto_add = false;
    }
    let v = rows();
    st.auto_add_to_cache(&v);
    assert!(global_cache().read().unwrap().entries.is_empty(), "auto-add off");
    global_cache().write().unwrap().auto_add = true;
    st.auto_add_to_cache(&v); // already scanned: nothing new
    assert!(global_cache().read().unwrap().entries.is_empty());
    st.auto_add_to_cache(&[]); // rows cleared: start over
    st.auto_add_to_cache(&v);
    let n = global_cache().read().unwrap().entries.len();
    assert_eq!(n, 3, "the two OpenAI calls and the Ollama call; the error is skipped");
    let _ = d;
    let mut g = global_cache().write().unwrap();
    g.auto_add = false;
    g.entries.clear();
}

#[test]
fn number_formatting() {
    assert_eq!(fmt_tokens(0), "0");
    assert_eq!(fmt_tokens(1234567), "1,234,567");
    assert_eq!(fmt_tokens(999), "999");
    assert_eq!(fmt_cost(None), "no price");
    assert_eq!(fmt_cost(Some(0.0)), "$0");
    assert_eq!(fmt_cost(Some(0.00123)), "$0.00123");
    assert_eq!(fmt_cost(Some(1.5)), "$1.5000");
}

// G11: token / cost math with the editable price table, totals, and grouping of agent conversations.
use ferrotap::llm_analysis::*;
use ferrotap::model::SessionRecord;
use std::collections::BTreeMap;

fn call(model: &str, provider: &str, input: u64, cached: u64, output: u64) -> LlmCall {
    LlmCall { model: model.into(), provider: provider.into(), usage: Usage { input, cached, output, reported: true, ..Default::default() }, ..Default::default() }
}

#[test]
fn cost_uses_longest_matching_pattern_and_cached_price() {
    let t = PriceTable {
        note: "test".into(),
        models: vec![
            ModelPrice { pattern: "gpt-4o".into(), input_per_m: 2.5, output_per_m: 10.0, cached_input_per_m: Some(1.25) },
            ModelPrice { pattern: "gpt-4o-mini".into(), input_per_m: 0.15, output_per_m: 0.6, cached_input_per_m: None },
        ],
    };
    // 1M input of which 400k cached, 100k output
    let c = t.cost(&call("gpt-4o-2024-08-06", "OpenAI", 1_000_000, 400_000, 100_000)).unwrap();
    assert!((c.input - 1.5).abs() < 1e-9);
    assert!((c.cached - 0.5).abs() < 1e-9);
    assert!((c.output - 1.0).abs() < 1e-9);
    assert!((c.total() - 3.0).abs() < 1e-9);
    // the longer pattern wins; cached falls back to the input price
    let c = t.cost(&call("GPT-4o-mini", "OpenAI", 2_000_000, 1_000_000, 1_000_000)).unwrap();
    assert!((c.total() - (0.3 + 0.6)).abs() < 1e-9);
    // unknown model: no price; local provider: free
    assert!(t.cost(&call("mystery-1", "OpenAI", 10, 0, 10)).is_none());
    assert_eq!(t.cost(&call("mystery-1", "Ollama (local)", 10, 0, 10)).unwrap().total(), 0.0);

    let tot = totals([call("gpt-4o", "OpenAI", 1_000_000, 0, 0), call("x", "OpenAI", 5, 0, 5), call("gpt-4o-mini", "OpenAI", 0, 0, 1_000_000)].iter(), &t);
    assert_eq!((tot.calls, tot.input, tot.output, tot.unpriced), (3, 1_000_005, 1_000_005, 1));
    assert!((tot.cost - 3.1).abs() < 1e-9);
}

#[test]
fn default_prices_are_marked_estimates_and_round_trip() {
    let d = PriceTable::default();
    assert!(d.note.contains("ESTIMATE"));
    assert!(d.find("claude-sonnet-4-5-20250929").is_some());
    assert!(d.find("gemini-2.5-flash-lite").unwrap().pattern == "gemini-2.5-flash-lite");
    assert_eq!(d.find("gpt-4o-mini-2024-07-18").unwrap().pattern, "gpt-4o-mini");
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("llm_prices.json");
    // missing file -> defaults
    assert_eq!(PriceTable::load(&p), d);
    let mut edited = d.clone();
    edited.models[0].input_per_m = 9.99;
    edited.save(&p).unwrap();
    assert_eq!(PriceTable::load(&p), edited);
}

fn chat(id: usize, msgs: &str, reply: &str, system: &str) -> SessionRecord {
    let req = format!(r#"{{"model":"claude-sonnet-4-5","max_tokens":512,"system":"{system}","messages":[{msgs}]}}"#);
    SessionRecord {
        id,
        method: "POST".into(),
        url: "https://api.anthropic.com/v1/messages".into(),
        status: 200,
        response_headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        request_body: req,
        response_body: format!(r#"{{"model":"claude-sonnet-4-5","content":{reply},"stop_reason":"end_turn","usage":{{"input_tokens":{},"output_tokens":10}}}}"#, 100 * id),
        duration_ms: 1000,
        ..Default::default()
    }
}

#[test]
fn agent_steps_are_grouped_by_growing_conversation() {
    let u1 = r#"{"role":"user","content":"Fix the bug"}"#;
    let a1 = r#"{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"read","input":{"p":"a.rs"}}]}"#;
    let r1 = r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"code"}]}"#;
    let a2 = r#"{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"write","input":{"p":"a.rs"}}]}"#;
    let r2 = r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]}"#;
    let tool1 = r#"[{"type":"tool_use","id":"t1","name":"read","input":{"p":"a.rs"}}]"#;
    let rows = vec![
        chat(1, u1, tool1, "agent"),
        // an unrelated chat interleaved with the agent's steps
        chat(2, r#"{"role":"user","content":"Hello"}"#, r#"[{"type":"text","text":"Hi"}]"#, "chat"),
        chat(3, &format!("{u1},{a1},{r1}"), r#"[{"type":"tool_use","id":"t2","name":"write","input":{"p":"a.rs"}}]"#, "agent"),
        chat(4, &format!("{u1},{a1},{r1},{a2},{r2}"), r#"[{"type":"text","text":"Fixed."}]"#, "agent"),
        // same first message but a different system prompt: another conversation
        chat(5, &format!("{u1},{a1},{r1}"), r#"[{"type":"text","text":"x"}]"#, "other agent"),
        // the same task started again from scratch: a new group
        chat(6, u1, tool1, "agent"),
    ];
    let calls: Vec<LlmCall> = rows.iter().map(|r| parse(r).unwrap()).collect();
    assert!(continues(&calls[0], &calls[2]));
    assert!(!continues(&calls[2], &calls[0]));
    let groups = group_agents(&calls);
    let ids: Vec<Vec<usize>> = groups.iter().map(|g| g.ids.clone()).collect();
    assert_eq!(ids, vec![vec![1, 3, 4], vec![2], vec![5], vec![6]]);
    // per-group totals
    let steps: Vec<&LlmCall> = calls.iter().filter(|c| groups[0].ids.contains(&c.id)).collect();
    let t = totals(steps.into_iter(), &PriceTable::default());
    assert_eq!((t.calls, t.input, t.output), (3, 800, 30));
    // claude-sonnet: $3/M in, $15/M out
    assert!((t.cost - (800.0 * 3.0 + 30.0 * 15.0) / 1e6).abs() < 1e-12);
}

#[test]
fn openai_agent_with_tool_messages_groups_too() {
    let mk = |id: usize, msgs: &str| SessionRecord {
        id,
        method: "POST".into(),
        url: "https://openrouter.ai/api/v1/chat/completions".into(),
        status: 200,
        request_body: format!(r#"{{"model":"openai/gpt-4o","messages":[{{"role":"system","content":"agent"}},{msgs}]}}"#),
        response_body: r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#.into(),
        ..Default::default()
    };
    let u = r#"{"role":"user","content":"list files"}"#;
    // the client re-sends the assistant tool call with different JSON spacing: still the same message
    let a = r#"{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"ls","arguments":"{\"dir\": \".\"}"}}]}"#;
    let a_spaced = r#"{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"ls","arguments":"{\"dir\":\".\"}"}}]}"#;
    let t = r#"{"role":"tool","tool_call_id":"c1","content":"a.rs b.rs"}"#;
    let calls: Vec<LlmCall> = vec![mk(10, &format!("{u},{a}")), mk(11, &format!("{u},{a_spaced},{t}"))].iter().map(|r| parse(r).unwrap()).collect();
    assert_eq!(group_agents(&calls).len(), 1);
}

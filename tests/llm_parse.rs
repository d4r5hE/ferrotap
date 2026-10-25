// G11: detection and parsing of LLM API calls for each provider, plain JSON and streamed (SSE / NDJSON).
// Bodies are realistic hand-written captures; nothing here contacts a real AI API.
use ferrotap::llm_analysis::*;
use ferrotap::model::SessionRecord;
use std::collections::BTreeMap;

fn sess(id: usize, url: &str, req: &str, ct: &str, resp: &str) -> SessionRecord {
    let mut rh = BTreeMap::new();
    rh.insert("content-type".to_string(), ct.to_string());
    SessionRecord {
        id,
        method: "POST".into(),
        url: url.into(),
        status: 200,
        request_headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        response_headers: rh,
        request_body: req.into(),
        response_body: resp.into(),
        start_time: "2026-10-16T12:00:00.000+00:00".into(),
        duration_ms: 2000,
        ttfb_ms: Some(400),
        ..Default::default()
    }
}

const OPENAI_REQ: &str = r#"{
  "model": "gpt-4o-mini",
  "messages": [
    {"role": "system", "content": "You are a weather bot."},
    {"role": "user", "content": "Weather in Paris?"}
  ],
  "tools": [{"type": "function", "function": {"name": "get_weather", "description": "Current weather for a city", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
  "temperature": 0.2,
  "max_tokens": 300
}"#;

#[test]
fn openai_chat_non_streamed() {
    let resp = r#"{"id":"chatcmpl-1","object":"chat.completion","model":"gpt-4o-mini-2024-07-18",
      "choices":[{"index":0,"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\"}"}}]},
        "finish_reason":"tool_calls"}],
      "usage":{"prompt_tokens":120,"completion_tokens":18,"total_tokens":138,
        "prompt_tokens_details":{"cached_tokens":64},"completion_tokens_details":{"reasoning_tokens":0}}}"#;
    let r = sess(1, "https://api.openai.com/v1/chat/completions", OPENAI_REQ, "application/json", resp);
    assert_eq!(detect(&r), Some(ApiKind::ChatCompletions));
    let c = parse(&r).unwrap();
    assert_eq!(c.provider, "OpenAI");
    assert_eq!(c.model, "gpt-4o-mini-2024-07-18");
    assert_eq!(c.system, "You are a weather bot.");
    assert_eq!(c.messages.len(), 1);
    assert_eq!(c.messages[0].role, "user");
    assert_eq!(c.messages[0].text(), "Weather in Paris?");
    assert_eq!(c.tools, vec![ToolDef { name: "get_weather".into(), description: "Current weather for a city".into() }]);
    assert!(c.params.contains(&("temperature".to_string(), "0.2".to_string())));
    assert!(c.params.contains(&("max_tokens".to_string(), "300".to_string())));
    assert!(!c.streamed);
    assert_eq!(c.stop_reason, "tool_calls");
    assert_eq!(c.reply.parts, vec![Part::ToolCall(ToolCall { id: "call_1".into(), name: "get_weather".into(), arguments: r#"{"city":"Paris"}"#.into() })]);
    assert_eq!(c.usage, Usage { input: 120, output: 18, cached: 64, cache_write: 0, thinking: 0, reported: true });
    assert_eq!(c.ttft_ms, Some(400));
}

#[test]
fn openai_chat_streamed_with_usage_and_tool_call_deltas() {
    let req = OPENAI_REQ.replace("\"max_tokens\": 300", "\"max_tokens\": 300, \"stream\": true, \"stream_options\": {\"include_usage\": true}");
    let resp = "data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Checking \"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"now.\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Paris\\\"}\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: {\"id\":\"c\",\"model\":\"gpt-4o-mini-2024-07-18\",\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":25,\"total_tokens\":145}}\n\n\
data: [DONE]\n\n";
    let r = sess(2, "https://api.openai.com/v1/chat/completions", &req, "text/event-stream; charset=utf-8", resp);
    let c = parse(&r).unwrap();
    assert!(c.streamed);
    assert_eq!(c.reply_text(), "Checking now.");
    assert_eq!(c.stop_reason, "tool_calls");
    assert!(c.reply.parts.contains(&Part::ToolCall(ToolCall { id: "call_9".into(), name: "get_weather".into(), arguments: r#"{"city":"Paris"}"#.into() })));
    assert_eq!((c.usage.input, c.usage.output), (120, 25));
    // 25 tokens over 2000 - 400 ms of generation
    assert!((c.tokens_per_sec().unwrap() - 15.625).abs() < 1e-9);
}

#[test]
fn openai_compatible_providers_by_host() {
    let resp = r#"{"model":"m","choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":1}}"#;
    let req = r#"{"model":"m","messages":[{"role":"user","content":"hello"}]}"#;
    let cases = [
        ("https://openrouter.ai/api/v1/chat/completions", "OpenRouter"),
        ("https://api.groq.com/openai/v1/chat/completions", "Groq"),
        ("https://api.together.xyz/v1/chat/completions", "Together"),
        ("http://localhost:11434/v1/chat/completions", "Ollama (local)"),
        ("http://127.0.0.1:1234/v1/chat/completions", "LM Studio (local)"),
        ("https://llm.example.com/v1/chat/completions", "OpenAI-compatible (llm.example.com)"),
    ];
    for (url, name) in cases {
        let c = parse(&sess(3, url, req, "application/json", resp)).unwrap();
        assert_eq!(c.provider, name, "{url}");
        assert_eq!(c.reply_text(), "hi");
        assert_eq!(c.kind, Some(ApiKind::ChatCompletions));
    }
    // a GET, a non-JSON body or an unrelated path is not an LLM call
    let mut g = sess(4, "https://api.openai.com/v1/chat/completions", req, "application/json", resp);
    g.method = "GET".into();
    assert!(parse(&g).is_none());
    assert!(parse(&sess(5, "https://api.openai.com/v1/chat/completions", "not json", "", "")).is_none());
    assert!(parse(&sess(6, "https://shop.example.com/cart", req, "application/json", resp)).is_none());
}

#[test]
fn openai_responses_api_plain_and_streamed() {
    let req = r#"{"model":"gpt-5-mini","instructions":"Be brief.","input":[
        {"role":"user","content":[{"type":"input_text","text":"Add 2 and 3"}]},
        {"type":"function_call","call_id":"fc_1","name":"add","arguments":"{\"a\":2,\"b\":3}"},
        {"type":"function_call_output","call_id":"fc_1","output":"5"}],
      "tools":[{"type":"function","name":"add","description":"Adds numbers"}],"reasoning":{"effort":"low"}}"#;
    let resp = r#"{"id":"resp_1","object":"response","status":"completed","model":"gpt-5-mini-2025-08-07",
      "output":[{"type":"reasoning","summary":[]},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"2 + 3 = 5"}]}],
      "usage":{"input_tokens":80,"input_tokens_details":{"cached_tokens":0},"output_tokens":40,"output_tokens_details":{"reasoning_tokens":32},"total_tokens":120}}"#;
    let c = parse(&sess(7, "https://api.openai.com/v1/responses", req, "application/json", resp)).unwrap();
    assert_eq!(c.kind, Some(ApiKind::Responses));
    assert_eq!(c.system, "Be brief.");
    assert_eq!(c.messages.len(), 3);
    assert_eq!(c.messages[1].role, "assistant");
    assert!(matches!(&c.messages[1].parts[0], Part::ToolCall(t) if t.name == "add"));
    assert_eq!(c.messages[2].parts[0], Part::ToolResult { id: "fc_1".into(), content: "5".into() });
    assert_eq!(c.tools[0].name, "add");
    assert!(c.params.iter().any(|(k, v)| k == "reasoning" && v.contains("low")));
    assert_eq!(c.reply_text(), "2 + 3 = 5");
    assert_eq!(c.stop_reason, "completed");
    assert_eq!((c.usage.input, c.usage.output, c.usage.thinking), (80, 40, 32));

    let stream = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-5-mini\",\"status\":\"in_progress\"}}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"2 + 3\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\" = 5\"}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"model\":\"gpt-5-mini-2025-08-07\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"2 + 3 = 5\"}]}],\"usage\":{\"input_tokens\":80,\"output_tokens\":40,\"output_tokens_details\":{\"reasoning_tokens\":32}}}}\n\n";
    let c = parse(&sess(8, "https://api.openai.com/v1/responses", req, "text/event-stream", stream)).unwrap();
    assert!(c.streamed);
    assert_eq!(c.reply_text(), "2 + 3 = 5");
    assert_eq!((c.usage.input, c.usage.output, c.usage.thinking), (80, 40, 32));
    // a stream cut before response.completed still reassembles the deltas
    let cut: String = stream.split("event: response.completed").next().unwrap().to_string();
    let c = parse(&sess(9, "https://api.openai.com/v1/responses", req, "text/event-stream", &cut)).unwrap();
    assert_eq!(c.reply_text(), "2 + 3 = 5");
}

const ANTHROPIC_REQ: &str = r#"{"model":"claude-sonnet-4-5","max_tokens":1024,
  "system":[{"type":"text","text":"You are a coding agent.","cache_control":{"type":"ephemeral"}}],
  "tools":[{"name":"read_file","description":"Read a file","input_schema":{"type":"object"}}],
  "messages":[
    {"role":"user","content":"Open main.rs"},
    {"role":"assistant","content":[{"type":"text","text":"Reading it."},{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"main.rs"}}]},
    {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"fn main() {}"}]}],
  "temperature":0.5}"#;

#[test]
fn anthropic_messages_non_streamed() {
    let resp = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-5-20250929",
      "content":[{"type":"thinking","thinking":"It is empty.","signature":"x"},{"type":"text","text":"main.rs has an empty main."}],
      "stop_reason":"end_turn","usage":{"input_tokens":50,"cache_creation_input_tokens":200,"cache_read_input_tokens":1000,"output_tokens":30}}"#;
    let r = sess(10, "https://api.anthropic.com/v1/messages", ANTHROPIC_REQ, "application/json", resp);
    let c = parse(&r).unwrap();
    assert_eq!(c.kind, Some(ApiKind::AnthropicMessages));
    assert_eq!(c.provider, "Anthropic");
    assert_eq!(c.model, "claude-sonnet-4-5-20250929");
    assert_eq!(c.system, "You are a coding agent.");
    assert_eq!(c.messages.len(), 3);
    assert!(matches!(&c.messages[1].parts[1], Part::ToolCall(t) if t.name == "read_file" && t.arguments.contains("main.rs")));
    assert_eq!(c.messages[2].parts[0], Part::ToolResult { id: "toolu_1".into(), content: "fn main() {}".into() });
    assert_eq!(c.reply_text(), "main.rs has an empty main.");
    assert_eq!(c.reply.parts[0], Part::Thinking("It is empty.".into()));
    assert_eq!(c.stop_reason, "end_turn");
    assert_eq!(c.usage, Usage { input: 1250, output: 30, cached: 1000, cache_write: 200, thinking: 0, reported: true });
    assert!(c.params.contains(&("max_tokens".to_string(), "1024".to_string())));
}

#[test]
fn anthropic_messages_streamed() {
    let resp = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"model\":\"claude-sonnet-4-5-20250929\",\"content\":[],\"usage\":{\"input_tokens\":50,\"cache_read_input_tokens\":1000,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: ping\ndata: {\"type\":\"ping\"}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Let me \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"edit it.\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_2\",\"name\":\"write_file\",\"input\":{}}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\": \\\"ma\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"in.rs\\\"}\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":42}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let c = parse(&sess(11, "https://api.anthropic.com/v1/messages", ANTHROPIC_REQ, "text/event-stream", resp)).unwrap();
    assert!(c.streamed);
    assert_eq!(c.reply_text(), "Let me edit it.");
    assert_eq!(c.reply.parts[1], Part::ToolCall(ToolCall { id: "toolu_2".into(), name: "write_file".into(), arguments: r#"{"path": "main.rs"}"#.into() }));
    assert_eq!(c.stop_reason, "tool_use");
    assert_eq!((c.usage.input, c.usage.cached, c.usage.output), (1050, 1000, 42));
}

const GEMINI_REQ: &str = r#"{"systemInstruction":{"parts":[{"text":"Answer in one line."}]},
  "contents":[{"role":"user","parts":[{"text":"Why is the sky blue?"}]}],
  "tools":[{"functionDeclarations":[{"name":"search","description":"Web search"}]}],
  "generationConfig":{"temperature":0.7,"maxOutputTokens":256}}"#;

#[test]
fn gemini_generate_content_plain() {
    let resp = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Rayleigh scattering of sunlight."}]},"finishReason":"STOP"}],
      "usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":7,"thoughtsTokenCount":100,"cachedContentTokenCount":4,"totalTokenCount":127},
      "modelVersion":"gemini-2.5-flash"}"#;
    let url = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent";
    let c = parse(&sess(12, url, GEMINI_REQ, "application/json; charset=UTF-8", resp)).unwrap();
    assert_eq!(c.kind, Some(ApiKind::Gemini));
    assert_eq!(c.provider, "Google Gemini");
    assert_eq!(c.model, "gemini-2.5-flash");
    assert_eq!(c.system, "Answer in one line.");
    assert_eq!(c.messages[0].text(), "Why is the sky blue?");
    assert_eq!(c.tools[0].name, "search");
    assert!(c.params.contains(&("maxOutputTokens".to_string(), "256".to_string())));
    assert_eq!(c.reply_text(), "Rayleigh scattering of sunlight.");
    assert_eq!(c.stop_reason, "STOP");
    assert_eq!(c.usage, Usage { input: 20, output: 107, cached: 4, cache_write: 0, thinking: 100, reported: true });
}

#[test]
fn gemini_stream_sse_and_json_array() {
    let url = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse";
    let sse = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Rayleigh \"}]}}],\"usageMetadata\":{\"promptTokenCount\":20},\"modelVersion\":\"gemini-2.5-flash\"}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"scattering.\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":5,\"totalTokenCount\":25},\"modelVersion\":\"gemini-2.5-flash\"}\r\n\r\n";
    let c = parse(&sess(13, url, GEMINI_REQ, "text/event-stream", sse)).unwrap();
    assert!(c.streamed);
    assert_eq!(c.reply_text(), "Rayleigh scattering.");
    assert_eq!((c.usage.input, c.usage.output), (20, 5));
    // without alt=sse the stream is one JSON array of chunks
    let arr = "[{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Ray\"}]}}]},\n{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"leigh\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":2}}]";
    let url2 = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent";
    let c = parse(&sess(14, url2, GEMINI_REQ, "application/json", arr)).unwrap();
    assert_eq!(c.reply_text(), "Rayleigh");
    assert_eq!(c.usage.output, 2);
}

#[test]
fn ollama_native_chat_streamed_ndjson_and_plain() {
    let req = r#"{"model":"llama3.2","messages":[{"role":"user","content":"Hi"}],"options":{"temperature":0.1}}"#;
    let nd = "{\"model\":\"llama3.2\",\"created_at\":\"t\",\"message\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"done\":false}\n\
{\"model\":\"llama3.2\",\"created_at\":\"t\",\"message\":{\"role\":\"assistant\",\"content\":\"lo!\"},\"done\":false}\n\
{\"model\":\"llama3.2\",\"created_at\":\"t\",\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":26,\"eval_count\":3}\n";
    let c = parse(&sess(15, "http://localhost:11434/api/chat", req, "application/x-ndjson", nd)).unwrap();
    assert_eq!(c.kind, Some(ApiKind::OllamaChat));
    assert_eq!(c.provider, "Ollama (local)");
    assert!(c.streamed);
    assert_eq!(c.reply_text(), "Hello!");
    assert_eq!(c.stop_reason, "stop");
    assert_eq!((c.usage.input, c.usage.output), (26, 3));
    assert!(c.params.contains(&("temperature".to_string(), "0.1".to_string())));
    let plain = r#"{"model":"llama3.2","message":{"role":"assistant","content":"Hello!"},"done":true,"done_reason":"stop","prompt_eval_count":26,"eval_count":3}"#;
    let c = parse(&sess(16, "http://localhost:11434/api/chat", req, "application/json; charset=utf-8", plain)).unwrap();
    assert!(!c.streamed);
    assert_eq!(c.reply_text(), "Hello!");
    // /api/generate
    let g = parse(&sess(17, "http://127.0.0.1:11434/api/generate", r#"{"model":"llama3.2","prompt":"Say hi","system":"terse","stream":false}"#, "application/json", r#"{"model":"llama3.2","response":"hi","done":true,"prompt_eval_count":4,"eval_count":1}"#)).unwrap();
    assert_eq!(g.kind, Some(ApiKind::OllamaGenerate));
    assert_eq!((g.system.as_str(), g.messages[0].text(), g.reply_text()), ("terse", "Say hi".to_string(), "hi".to_string()));
}

#[test]
fn error_responses_are_reported() {
    let mut r = sess(18, "https://api.anthropic.com/v1/messages", ANTHROPIC_REQ, "application/json", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
    r.status = 529;
    let c = parse(&r).unwrap();
    assert_eq!(c.error.as_deref(), Some("Overloaded"));
    assert!(!c.usage.reported);
    // no response yet (still pending / connection failed): still recognized from the request
    let c = parse(&sess(19, "https://api.openai.com/v1/chat/completions", OPENAI_REQ, "", "")).unwrap();
    assert_eq!(c.model, "gpt-4o-mini");
    assert!(c.reply.parts.is_empty());
}

//! G11 analysis of captured LLM API calls (no UI): detect the provider / API format of a session, parse the
//! request (model, system prompt, messages, tools, sampling parameters) and the response (plain JSON or a
//! streamed SSE / NDJSON body, reassembled), token usage, latency, cost from an editable price table,
//! "agent call" grouping, and the LLM response cache used by the proxy hook.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::SessionRecord;

// ===================== Detection =====================

/// The wire format of an LLM API call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKind {
    /// OpenAI-compatible `/chat/completions` (OpenAI, OpenRouter, Groq, Together, Ollama /v1, LM Studio, ...)
    ChatCompletions,
    /// OpenAI legacy `/completions` (prompt in, text out)
    Completions,
    /// OpenAI Responses API `/responses`
    Responses,
    /// Anthropic Messages `/v1/messages`
    AnthropicMessages,
    /// Google Gemini `:generateContent` / `:streamGenerateContent`
    Gemini,
    /// Ollama native `/api/chat`
    OllamaChat,
    /// Ollama native `/api/generate`
    OllamaGenerate,
}

impl ApiKind {
    pub fn label(&self) -> &'static str {
        match self {
            ApiKind::ChatCompletions => "Chat Completions",
            ApiKind::Completions => "Completions",
            ApiKind::Responses => "Responses",
            ApiKind::AnthropicMessages => "Anthropic Messages",
            ApiKind::Gemini => "Gemini generateContent",
            ApiKind::OllamaChat => "Ollama /api/chat",
            ApiKind::OllamaGenerate => "Ollama /api/generate",
        }
    }
}

fn path_only(r: &SessionRecord) -> &str {
    let p = r.path();
    &p[..p.find('?').unwrap_or(p.len())]
}

/// The host without the port, lowercased.
fn host_name(r: &SessionRecord) -> String {
    let h = r.host();
    let h = h.rsplit('@').next().unwrap_or(h);
    let h = if h.starts_with('[') { h.split(']').next().unwrap_or(h).trim_start_matches('[') } else { h.split(':').next().unwrap_or(h) };
    h.to_ascii_lowercase()
}

fn port(r: &SessionRecord) -> Option<u16> {
    let h = r.host();
    let tail = if h.starts_with('[') { h.rsplit("]:").next().filter(|t| !t.contains(']')) } else { h.rsplit_once(':').map(|x| x.1) };
    tail.and_then(|p| p.parse().ok())
}

/// The API format of a method + URL + request body, or None when it is not an LLM API call.
pub fn detect_parts(method: &str, url: &str, request_body: &str) -> Option<ApiKind> {
    if !method.eq_ignore_ascii_case("POST") {
        return None;
    }
    let r = SessionRecord { url: url.to_string(), ..Default::default() };
    let path = path_only(&r).trim_end_matches('/').to_ascii_lowercase();
    let host = host_name(&r);
    let body: Value = serde_json::from_str(request_body.trim()).ok()?;
    let obj = body.as_object()?;
    let has = |k: &str| obj.contains_key(k);
    if path.contains(":generatecontent") || path.contains(":streamgeneratecontent") {
        return has("contents").then_some(ApiKind::Gemini);
    }
    if path.ends_with("/chat/completions") {
        return has("messages").then_some(ApiKind::ChatCompletions);
    }
    if path.ends_with("/completions") {
        return has("prompt").then_some(ApiKind::Completions);
    }
    if path.ends_with("/responses") {
        return (has("input") || has("model")).then_some(ApiKind::Responses);
    }
    if path.ends_with("/v1/messages") || (host.contains("anthropic") && path.ends_with("/messages")) {
        return has("messages").then_some(ApiKind::AnthropicMessages);
    }
    if path.ends_with("/api/chat") {
        return has("messages").then_some(ApiKind::OllamaChat);
    }
    if path.ends_with("/api/generate") {
        return (has("prompt") && has("model")).then_some(ApiKind::OllamaGenerate);
    }
    None
}

pub fn detect(r: &SessionRecord) -> Option<ApiKind> {
    detect_parts(&r.method, &r.url, &r.request_body)
}

/// A readable provider name from the host (and API format).
pub fn provider_name(r: &SessionRecord, kind: ApiKind) -> String {
    let h = host_name(r);
    let named = [
        ("api.openai.com", "OpenAI"),
        ("openai.azure.com", "Azure OpenAI"),
        ("openrouter.ai", "OpenRouter"),
        ("api.groq.com", "Groq"),
        ("together.xyz", "Together"),
        ("together.ai", "Together"),
        ("api.anthropic.com", "Anthropic"),
        ("generativelanguage.googleapis.com", "Google Gemini"),
        ("aiplatform.googleapis.com", "Google Vertex AI"),
        ("api.mistral.ai", "Mistral"),
        ("api.deepseek.com", "DeepSeek"),
        ("api.x.ai", "xAI"),
        ("api.fireworks.ai", "Fireworks"),
        ("api.perplexity.ai", "Perplexity"),
    ];
    for (needle, name) in named {
        if h == needle || h.ends_with(&format!(".{needle}")) || h.ends_with(needle) {
            return name.to_string();
        }
    }
    let local = matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") || h.ends_with(".local");
    if matches!(kind, ApiKind::OllamaChat | ApiKind::OllamaGenerate) || port(r) == Some(11434) || h.contains("ollama") {
        return "Ollama (local)".into();
    }
    if port(r) == Some(1234) || h.contains("lmstudio") {
        return "LM Studio (local)".into();
    }
    if local {
        return format!("Local server ({})", r.host());
    }
    match kind {
        ApiKind::AnthropicMessages => format!("Anthropic-compatible ({h})"),
        ApiKind::Gemini => format!("Gemini-compatible ({h})"),
        _ => format!("OpenAI-compatible ({h})"),
    }
}

/// True for providers that run on the user's own machine (no cost).
pub fn is_local_provider(provider: &str) -> bool {
    provider.contains("(local)") || provider.starts_with("Local server")
}

// ===================== Parsed call =====================

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Thinking(String),
    ToolCall(ToolCall),
    ToolResult { id: String, content: String },
    /// Images, audio, files: just a short description.
    Media(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    pub role: String,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn text(&self) -> String {
        let mut out = Vec::new();
        for p in &self.parts {
            if let Part::Text(t) = p {
                out.push(t.as_str());
            }
        }
        out.join("\n")
    }

    /// A stable signature used to compare conversations (agent grouping).
    pub fn signature(&self) -> String {
        let mut s = format!("{}|", normalize_role(&self.role));
        for p in &self.parts {
            match p {
                Part::Text(t) => s.push_str(&format!("t:{}|", t.trim())),
                Part::Thinking(_) => {}
                Part::ToolCall(c) => s.push_str(&format!("c:{}:{}|", c.name, canonical_json_str(&c.arguments))),
                Part::ToolResult { id, content } => s.push_str(&format!("r:{id}:{}|", content.trim())),
                Part::Media(m) => s.push_str(&format!("m:{m}|")),
            }
        }
        s
    }
}

fn normalize_role(r: &str) -> &str {
    match r {
        "model" => "assistant",
        "function" => "tool",
        x => x,
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
}

/// Token counts. `input` is the whole prompt including cached tokens; `output` includes thinking tokens.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    /// Prompt tokens read from the provider's prompt cache (part of `input`).
    pub cached: u64,
    /// Prompt tokens written to the provider's prompt cache (Anthropic; part of `input`).
    pub cache_write: u64,
    /// Reasoning / thinking tokens (part of `output`), when the provider reports them.
    pub thinking: u64,
    /// True when the response carried usage numbers at all.
    pub reported: bool,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LlmCall {
    pub id: usize,
    pub kind: Option<ApiKind>,
    pub provider: String,
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    /// Sampling and other request parameters (name, compact value).
    pub params: Vec<(String, String)>,
    pub streamed: bool,
    /// The assistant reply (text, thinking, tool calls).
    pub reply: Message,
    pub stop_reason: String,
    pub usage: Usage,
    pub status: u16,
    pub error: Option<String>,
    pub ttft_ms: Option<u64>,
    pub duration_ms: u128,
    pub start_time: String,
}

impl LlmCall {
    pub fn reply_text(&self) -> String {
        self.reply.text()
    }

    /// Output tokens per second over the generation time (after the first token when streamed).
    pub fn tokens_per_sec(&self) -> Option<f64> {
        if self.usage.output == 0 {
            return None;
        }
        let total = self.duration_ms as f64;
        let gen = match self.ttft_ms {
            Some(t) if self.streamed && (t as f64) < total => total - t as f64,
            _ => total,
        };
        (gen > 0.0).then(|| self.usage.output as f64 * 1000.0 / gen)
    }
}

// ===================== JSON helpers =====================

fn s(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(x) => x.clone(),
        other => other.to_string(),
    }
}

fn get_u(v: &Value, path: &[&str]) -> u64 {
    let mut cur = v;
    for k in path {
        match cur.get(*k) {
            Some(x) => cur = x,
            None => return 0,
        }
    }
    cur.as_u64().unwrap_or(0)
}

fn has_path(v: &Value, path: &[&str]) -> bool {
    let mut cur = v;
    for k in path {
        match cur.get(*k) {
            Some(x) => cur = x,
            None => return false,
        }
    }
    !cur.is_null()
}

fn compact(v: &Value, max: usize) -> String {
    let t = s(v);
    if t.chars().count() > max {
        format!("{}...", t.chars().take(max).collect::<String>())
    } else {
        t
    }
}

/// JSON with object keys sorted at every level (whitespace-free), for stable comparisons.
pub fn canonical_json(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: BTreeMap<String, Value> = m.iter().map(|(k, v)| (k.clone(), sort(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            x => x.clone(),
        }
    }
    let mut out = String::new();
    write_canon(&sort(v), &mut out);
    out
}

fn write_canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*k).clone()).to_string());
                out.push(':');
                write_canon(&m[*k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canon(x, out);
            }
            out.push(']');
        }
        x => out.push_str(&x.to_string()),
    }
}

fn canonical_json_str(t: &str) -> String {
    serde_json::from_str::<Value>(t).map(|v| canonical_json(&v)).unwrap_or_else(|_| t.trim().to_string())
}

/// Text of a content value: a string, or an array of blocks with `text` fields.
fn content_text(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(String::from).or_else(|| b.as_str().map(String::from))).collect::<Vec<_>>().join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The JSON values carried by an SSE body ("data:" lines) or an NDJSON body, or a JSON array body.
pub fn stream_events(body: &str) -> Vec<Value> {
    let t = body.trim_start();
    if t.starts_with('[') {
        if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(t) {
            return a;
        }
    }
    let mut out = Vec::new();
    let mut data = String::new();
    let mut saw_data = false;
    let flush = |data: &mut String, out: &mut Vec<Value>| {
        let d = data.trim();
        if !d.is_empty() && d != "[DONE]" {
            if let Ok(v) = serde_json::from_str::<Value>(d) {
                out.push(v);
            }
        }
        data.clear();
    };
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            saw_data = true;
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            // most providers send one JSON per data line; join continuation lines otherwise
            if !data.is_empty() && serde_json::from_str::<Value>(data.trim()).is_ok() {
                flush(&mut data, &mut out);
            }
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        } else if line.is_empty() {
            flush(&mut data, &mut out);
        } else if !saw_data && (line.starts_with('{')) {
            // NDJSON (Ollama native streaming)
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                out.push(v);
            }
        }
    }
    flush(&mut data, &mut out);
    out
}

fn is_streamed_body(r: &SessionRecord) -> bool {
    let ct = r.content_type().to_ascii_lowercase();
    if ct.contains("event-stream") || ct.contains("ndjson") || ct.contains("jsonl") {
        return true;
    }
    let b = r.response_body.trim_start();
    b.starts_with("data:") || b.starts_with("event:") || (b.starts_with('{') && b.trim_end().lines().count() > 1 && b.lines().all(|l| l.trim().is_empty() || serde_json::from_str::<Value>(l.trim()).is_ok()))
}

// ===================== Request parsing =====================

const NOT_PARAMS: &[&str] = &["model", "messages", "tools", "system", "input", "instructions", "contents", "systemInstruction", "system_instruction", "functions", "prompt", "generationConfig", "generation_config", "options", "safetySettings", "tool_config", "toolConfig", "images"];

fn top_params(obj: &serde_json::Map<String, Value>, out: &mut Vec<(String, String)>) {
    for (k, v) in obj {
        if !NOT_PARAMS.contains(&k.as_str()) {
            out.push((k.clone(), compact(v, 160)));
        }
    }
    for nested in ["generationConfig", "generation_config", "options"] {
        if let Some(Value::Object(m)) = obj.get(nested) {
            for (k, v) in m {
                out.push((k.clone(), compact(v, 160)));
            }
        }
    }
}

fn openai_tools(v: Option<&Value>, out: &mut Vec<ToolDef>) {
    for t in v.and_then(|t| t.as_array()).into_iter().flatten() {
        let f = t.get("function").unwrap_or(t);
        let name = f.get("name").map(s).unwrap_or_default();
        let name = if name.is_empty() { t.get("type").map(s).unwrap_or_default() } else { name };
        out.push(ToolDef { name, description: f.get("description").map(s).unwrap_or_default() });
    }
}

fn openai_tool_calls(v: Option<&Value>) -> Vec<Part> {
    let mut parts = Vec::new();
    for c in v.and_then(|t| t.as_array()).into_iter().flatten() {
        let f = c.get("function").unwrap_or(c);
        parts.push(Part::ToolCall(ToolCall { id: c.get("id").map(s).unwrap_or_default(), name: f.get("name").map(s).unwrap_or_default(), arguments: f.get("arguments").map(s).unwrap_or_default() }));
    }
    parts
}

fn openai_content_parts(v: Option<&Value>) -> Vec<Part> {
    let mut parts = Vec::new();
    match v {
        Some(Value::String(t)) => parts.push(Part::Text(t.clone())),
        Some(Value::Array(a)) => {
            for b in a {
                let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    parts.push(Part::Text(t.to_string()));
                } else if let Some(t) = b.as_str() {
                    parts.push(Part::Text(t.to_string()));
                } else if !ty.is_empty() {
                    parts.push(Part::Media(ty.to_string()));
                }
            }
        }
        _ => {}
    }
    parts
}

fn parse_chat_request(obj: &serde_json::Map<String, Value>, call: &mut LlmCall) {
    let mut systems = Vec::new();
    for m in obj.get("messages").and_then(|m| m.as_array()).into_iter().flatten() {
        let role = m.get("role").map(s).unwrap_or_default();
        if role == "system" || role == "developer" {
            systems.push(content_text(m.get("content").unwrap_or(&Value::Null)));
            continue;
        }
        let mut parts = Vec::new();
        if role == "tool" || role == "function" {
            let id = m.get("tool_call_id").or_else(|| m.get("name")).map(s).unwrap_or_default();
            parts.push(Part::ToolResult { id, content: content_text(m.get("content").unwrap_or(&Value::Null)) });
        } else {
            if let Some(r) = m.get("reasoning_content").or_else(|| m.get("thinking")).and_then(|x| x.as_str()) {
                parts.push(Part::Thinking(r.to_string()));
            }
            parts.extend(openai_content_parts(m.get("content")));
            parts.extend(openai_tool_calls(m.get("tool_calls")));
            if let Some(fc) = m.get("function_call") {
                parts.push(Part::ToolCall(ToolCall { id: String::new(), name: fc.get("name").map(s).unwrap_or_default(), arguments: fc.get("arguments").map(s).unwrap_or_default() }));
            }
            if m.get("images").is_some() {
                parts.push(Part::Media("images".into()));
            }
        }
        call.messages.push(Message { role, parts });
    }
    call.system = systems.join("\n\n");
    openai_tools(obj.get("tools").or_else(|| obj.get("functions")), &mut call.tools);
}

fn parse_responses_request(obj: &serde_json::Map<String, Value>, call: &mut LlmCall) {
    call.system = obj.get("instructions").map(content_text).unwrap_or_default();
    match obj.get("input") {
        Some(Value::String(t)) => call.messages.push(Message { role: "user".into(), parts: vec![Part::Text(t.clone())] }),
        Some(Value::Array(items)) => {
            for it in items {
                let ty = it.get("type").and_then(|t| t.as_str()).unwrap_or("message");
                match ty {
                    "function_call" | "custom_tool_call" => call.messages.push(Message {
                        role: "assistant".into(),
                        parts: vec![Part::ToolCall(ToolCall { id: it.get("call_id").map(s).unwrap_or_default(), name: it.get("name").map(s).unwrap_or_default(), arguments: it.get("arguments").or_else(|| it.get("input")).map(s).unwrap_or_default() })],
                    }),
                    "function_call_output" | "custom_tool_call_output" => call.messages.push(Message {
                        role: "tool".into(),
                        parts: vec![Part::ToolResult { id: it.get("call_id").map(s).unwrap_or_default(), content: content_text(it.get("output").unwrap_or(&Value::Null)) }],
                    }),
                    "reasoning" => {}
                    _ => {
                        let role = it.get("role").map(s).unwrap_or_else(|| "user".into());
                        if role == "system" || role == "developer" {
                            let t = content_text(it.get("content").unwrap_or(&Value::Null));
                            call.system = if call.system.is_empty() { t } else { format!("{}\n\n{t}", call.system) };
                            continue;
                        }
                        call.messages.push(Message { role, parts: openai_content_parts(it.get("content")) });
                    }
                }
            }
        }
        _ => {}
    }
    openai_tools(obj.get("tools"), &mut call.tools);
}

fn anthropic_blocks(v: Option<&Value>) -> Vec<Part> {
    let mut parts = Vec::new();
    match v {
        Some(Value::String(t)) => parts.push(Part::Text(t.clone())),
        Some(Value::Array(a)) => {
            for b in a {
                match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text" => parts.push(Part::Text(b.get("text").map(s).unwrap_or_default())),
                    "thinking" => parts.push(Part::Thinking(b.get("thinking").map(s).unwrap_or_default())),
                    "redacted_thinking" => parts.push(Part::Thinking("(redacted thinking)".into())),
                    "tool_use" | "server_tool_use" => parts.push(Part::ToolCall(ToolCall { id: b.get("id").map(s).unwrap_or_default(), name: b.get("name").map(s).unwrap_or_default(), arguments: b.get("input").map(|i| i.to_string()).unwrap_or_default() })),
                    "tool_result" => parts.push(Part::ToolResult { id: b.get("tool_use_id").map(s).unwrap_or_default(), content: content_text(b.get("content").unwrap_or(&Value::Null)) }),
                    other => parts.push(Part::Media(other.to_string())),
                }
            }
        }
        _ => {}
    }
    parts
}

fn parse_anthropic_request(obj: &serde_json::Map<String, Value>, call: &mut LlmCall) {
    call.system = obj.get("system").map(content_text).unwrap_or_default();
    for m in obj.get("messages").and_then(|m| m.as_array()).into_iter().flatten() {
        let role = m.get("role").map(s).unwrap_or_default();
        call.messages.push(Message { role, parts: anthropic_blocks(m.get("content")) });
    }
    openai_tools(obj.get("tools"), &mut call.tools);
}

fn gemini_parts(v: Option<&Value>) -> Vec<Part> {
    let mut parts = Vec::new();
    for p in v.and_then(|x| x.as_array()).into_iter().flatten() {
        if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
            if p.get("thought").and_then(|b| b.as_bool()) == Some(true) {
                parts.push(Part::Thinking(t.to_string()));
            } else {
                parts.push(Part::Text(t.to_string()));
            }
        } else if let Some(fc) = p.get("functionCall") {
            parts.push(Part::ToolCall(ToolCall { id: fc.get("id").map(s).unwrap_or_default(), name: fc.get("name").map(s).unwrap_or_default(), arguments: fc.get("args").map(|a| a.to_string()).unwrap_or_default() }));
        } else if let Some(fr) = p.get("functionResponse") {
            parts.push(Part::ToolResult { id: fr.get("name").map(s).unwrap_or_default(), content: fr.get("response").map(|a| a.to_string()).unwrap_or_default() });
        } else if p.get("inlineData").is_some() || p.get("fileData").is_some() {
            parts.push(Part::Media("inline data".into()));
        }
    }
    parts
}

fn parse_gemini_request(r: &SessionRecord, obj: &serde_json::Map<String, Value>, call: &mut LlmCall) {
    let path = path_only(r);
    if let Some(i) = path.find("/models/") {
        let rest = &path[i + 8..];
        call.model = rest.split(':').next().unwrap_or(rest).to_string();
    }
    let si = obj.get("systemInstruction").or_else(|| obj.get("system_instruction"));
    if let Some(si) = si {
        call.system = gemini_parts(si.get("parts")).iter().filter_map(|p| if let Part::Text(t) = p { Some(t.clone()) } else { None }).collect::<Vec<_>>().join("\n");
    }
    for c in obj.get("contents").and_then(|m| m.as_array()).into_iter().flatten() {
        let role = c.get("role").map(s).unwrap_or_else(|| "user".into());
        let parts = gemini_parts(c.get("parts"));
        let role = if parts.iter().all(|p| matches!(p, Part::ToolResult { .. })) && !parts.is_empty() { "tool".to_string() } else { normalize_role(&role).to_string() };
        call.messages.push(Message { role, parts });
    }
    for t in obj.get("tools").and_then(|t| t.as_array()).into_iter().flatten() {
        let decls = t.get("functionDeclarations").or_else(|| t.get("function_declarations"));
        match decls.and_then(|d| d.as_array()) {
            Some(list) => {
                for d in list {
                    call.tools.push(ToolDef { name: d.get("name").map(s).unwrap_or_default(), description: d.get("description").map(s).unwrap_or_default() });
                }
            }
            None => {
                if let Some(k) = t.as_object().and_then(|o| o.keys().next()) {
                    call.tools.push(ToolDef { name: k.clone(), description: "built-in tool".into() });
                }
            }
        }
    }
}

fn parse_ollama_generate_request(obj: &serde_json::Map<String, Value>, call: &mut LlmCall) {
    call.system = obj.get("system").map(s).unwrap_or_default();
    call.messages.push(Message { role: "user".into(), parts: vec![Part::Text(obj.get("prompt").map(s).unwrap_or_default())] });
}

// ===================== Response parsing =====================

fn openai_usage(u: &Value) -> Usage {
    // Chat Completions uses prompt_/completion_tokens; the Responses API uses input_/output_tokens.
    let input = get_u(u, &["prompt_tokens"]).max(get_u(u, &["input_tokens"]));
    let output = get_u(u, &["completion_tokens"]).max(get_u(u, &["output_tokens"]));
    let cached = get_u(u, &["prompt_tokens_details", "cached_tokens"]).max(get_u(u, &["input_tokens_details", "cached_tokens"])).max(get_u(u, &["prompt_cache_hit_tokens"]));
    let thinking = get_u(u, &["completion_tokens_details", "reasoning_tokens"]).max(get_u(u, &["output_tokens_details", "reasoning_tokens"]));
    Usage { input, output, cached, cache_write: 0, thinking, reported: u.is_object() }
}

fn anthropic_usage(u: &Value, into: &mut Usage) {
    let base = get_u(u, &["input_tokens"]);
    let read = get_u(u, &["cache_read_input_tokens"]);
    let write = get_u(u, &["cache_creation_input_tokens"]);
    if has_path(u, &["input_tokens"]) || has_path(u, &["cache_read_input_tokens"]) {
        into.input = base + read + write;
        into.cached = read;
        into.cache_write = write;
    }
    if has_path(u, &["output_tokens"]) {
        into.output = get_u(u, &["output_tokens"]);
    }
    into.reported = true;
}

fn gemini_usage(u: &Value) -> Usage {
    let thinking = get_u(u, &["thoughtsTokenCount"]);
    Usage { input: get_u(u, &["promptTokenCount"]), output: get_u(u, &["candidatesTokenCount"]) + thinking, cached: get_u(u, &["cachedContentTokenCount"]), cache_write: 0, thinking, reported: true }
}

fn error_text(v: &Value) -> Option<String> {
    let e = v.get("error")?;
    Some(match e {
        Value::String(t) => t.clone(),
        o => o.get("message").map(s).unwrap_or_else(|| o.to_string()),
    })
}

fn parse_chat_response(v: &Value, call: &mut LlmCall) {
    if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
        call.model = m.to_string();
    }
    if let Some(c) = v.get("choices").and_then(|c| c.get(0)) {
        let mut parts = Vec::new();
        if let Some(m) = c.get("message") {
            if let Some(r) = m.get("reasoning_content").or_else(|| m.get("reasoning")).and_then(|x| x.as_str()) {
                parts.push(Part::Thinking(r.to_string()));
            }
            parts.extend(openai_content_parts(m.get("content")));
            parts.extend(openai_tool_calls(m.get("tool_calls")));
        } else if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
            parts.push(Part::Text(t.to_string()));
        }
        call.reply = Message { role: "assistant".into(), parts };
        call.stop_reason = c.get("finish_reason").map(s).unwrap_or_default();
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        call.usage = openai_usage(u);
    }
}

fn parse_responses_response(v: &Value, call: &mut LlmCall) {
    if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
        call.model = m.to_string();
    }
    let mut parts = Vec::new();
    for it in v.get("output").and_then(|o| o.as_array()).into_iter().flatten() {
        match it.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message" => parts.extend(openai_content_parts(it.get("content"))),
            "reasoning" => {
                let t = it.get("summary").map(content_text).unwrap_or_default();
                if !t.is_empty() {
                    parts.push(Part::Thinking(t));
                }
            }
            "function_call" | "custom_tool_call" => parts.push(Part::ToolCall(ToolCall { id: it.get("call_id").map(s).unwrap_or_default(), name: it.get("name").map(s).unwrap_or_default(), arguments: it.get("arguments").or_else(|| it.get("input")).map(s).unwrap_or_default() })),
            other if !other.is_empty() => parts.push(Part::Media(other.to_string())),
            _ => {}
        }
    }
    call.reply = Message { role: "assistant".into(), parts };
    let status = v.get("status").map(s).unwrap_or_default();
    let reason = v.get("incomplete_details").and_then(|d| d.get("reason")).map(s).unwrap_or_default();
    let has_call = call.reply.parts.iter().any(|p| matches!(p, Part::ToolCall(_)));
    call.stop_reason = if !reason.is_empty() { format!("{status}: {reason}") } else if has_call && status == "completed" { "completed (tool call)".into() } else { status };
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        call.usage = openai_usage(u);
    }
}

fn parse_anthropic_response(v: &Value, call: &mut LlmCall) {
    if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
        call.model = m.to_string();
    }
    call.reply = Message { role: "assistant".into(), parts: anthropic_blocks(v.get("content")) };
    call.stop_reason = v.get("stop_reason").map(s).unwrap_or_default();
    if let Some(u) = v.get("usage") {
        anthropic_usage(u, &mut call.usage);
    }
}

fn parse_gemini_chunk(v: &Value, call: &mut LlmCall, parts: &mut Vec<Part>) {
    if let Some(m) = v.get("modelVersion").and_then(|m| m.as_str()) {
        call.model = m.to_string();
    }
    if let Some(c) = v.get("candidates").and_then(|c| c.get(0)) {
        for p in gemini_parts(c.get("content").and_then(|c| c.get("parts"))) {
            push_merge(parts, p);
        }
        if let Some(fr) = c.get("finishReason").and_then(|f| f.as_str()) {
            call.stop_reason = fr.to_string();
        }
    }
    if let Some(u) = v.get("usageMetadata") {
        call.usage = gemini_usage(u);
    }
}

fn parse_ollama_chunk(v: &Value, call: &mut LlmCall, parts: &mut Vec<Part>) {
    if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
        call.model = m.to_string();
    }
    if let Some(m) = v.get("message") {
        if let Some(t) = m.get("thinking").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            push_merge(parts, Part::Thinking(t.to_string()));
        }
        if let Some(t) = m.get("content").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            push_merge(parts, Part::Text(t.to_string()));
        }
        for c in m.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten() {
            let f = c.get("function").unwrap_or(c);
            parts.push(Part::ToolCall(ToolCall { id: c.get("id").map(s).unwrap_or_default(), name: f.get("name").map(s).unwrap_or_default(), arguments: f.get("arguments").map(|a| if let Value::String(x) = a { x.clone() } else { a.to_string() }).unwrap_or_default() }));
        }
    }
    if let Some(t) = v.get("response").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
        push_merge(parts, Part::Text(t.to_string()));
    }
    if v.get("done").and_then(|d| d.as_bool()) == Some(true) {
        call.stop_reason = v.get("done_reason").map(s).unwrap_or_else(|| "stop".into());
        call.usage = Usage { input: get_u(v, &["prompt_eval_count"]), output: get_u(v, &["eval_count"]), reported: true, ..Default::default() };
    }
}

/// Appends text to the last part of the same kind, so streamed deltas become one block.
fn push_merge(parts: &mut Vec<Part>, p: Part) {
    match (parts.last_mut(), p) {
        (Some(Part::Text(a)), Part::Text(b)) => a.push_str(&b),
        (Some(Part::Thinking(a)), Part::Thinking(b)) => a.push_str(&b),
        (_, p) => parts.push(p),
    }
}

fn parse_chat_stream(events: &[Value], call: &mut LlmCall) {
    let mut parts: Vec<Part> = Vec::new();
    // tool calls by index: (id, name, arguments)
    let mut tools: BTreeMap<u64, ToolCall> = BTreeMap::new();
    for e in events {
        if let Some(m) = e.get("model").and_then(|m| m.as_str()).filter(|m| !m.is_empty()) {
            call.model = m.to_string();
        }
        if let Some(c) = e.get("choices").and_then(|c| c.get(0)) {
            let d = c.get("delta").unwrap_or(c);
            if let Some(t) = d.get("reasoning_content").or_else(|| d.get("reasoning")).and_then(|x| x.as_str()).filter(|t| !t.is_empty()) {
                push_merge(&mut parts, Part::Thinking(t.to_string()));
            }
            if let Some(t) = d.get("content").or_else(|| c.get("text")).and_then(|x| x.as_str()).filter(|t| !t.is_empty()) {
                push_merge(&mut parts, Part::Text(t.to_string()));
            }
            for tc in d.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten() {
                let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(tools.len() as u64);
                let e = tools.entry(idx).or_default();
                if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                    e.id = id.to_string();
                }
                if let Some(f) = tc.get("function") {
                    if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                        e.name.push_str(n);
                    }
                    if let Some(a) = f.get("arguments").and_then(|x| x.as_str()) {
                        e.arguments.push_str(a);
                    }
                }
            }
            if let Some(f) = c.get("finish_reason").and_then(|f| f.as_str()) {
                call.stop_reason = f.to_string();
            }
        }
        if let Some(u) = e.get("usage").filter(|u| u.is_object()) {
            call.usage = openai_usage(u);
        }
        if let Some(err) = error_text(e) {
            call.error = Some(err);
        }
    }
    parts.extend(tools.into_values().map(Part::ToolCall));
    call.reply = Message { role: "assistant".into(), parts };
}

fn parse_responses_stream(events: &[Value], call: &mut LlmCall) {
    // The final event carries the whole response; fall back to the deltas when the stream was cut short.
    for e in events.iter().rev() {
        let ty = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if matches!(ty, "response.completed" | "response.incomplete" | "response.failed") {
            if let Some(r) = e.get("response") {
                parse_responses_response(r, call);
                if let Some(err) = r.get("error").filter(|x| !x.is_null()) {
                    call.error = Some(err.get("message").map(s).unwrap_or_else(|| err.to_string()));
                }
                return;
            }
        }
    }
    let mut parts = Vec::new();
    for e in events {
        match e.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "response.created" | "response.in_progress" => {
                if let Some(m) = e.get("response").and_then(|r| r.get("model")).and_then(|m| m.as_str()) {
                    call.model = m.to_string();
                }
            }
            "response.output_text.delta" => push_merge(&mut parts, Part::Text(e.get("delta").map(s).unwrap_or_default())),
            "response.reasoning_summary_text.delta" => push_merge(&mut parts, Part::Thinking(e.get("delta").map(s).unwrap_or_default())),
            "response.output_item.done" => {
                if let Some(it) = e.get("item").filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("function_call")) {
                    parts.push(Part::ToolCall(ToolCall { id: it.get("call_id").map(s).unwrap_or_default(), name: it.get("name").map(s).unwrap_or_default(), arguments: it.get("arguments").map(s).unwrap_or_default() }));
                }
            }
            _ => {}
        }
    }
    call.stop_reason = "incomplete stream".into();
    call.reply = Message { role: "assistant".into(), parts };
}

fn parse_anthropic_stream(events: &[Value], call: &mut LlmCall) {
    let mut blocks: BTreeMap<u64, Part> = BTreeMap::new();
    let mut json_buf: BTreeMap<u64, String> = BTreeMap::new();
    for e in events {
        match e.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message_start" => {
                if let Some(m) = e.get("message") {
                    if let Some(model) = m.get("model").and_then(|x| x.as_str()) {
                        call.model = model.to_string();
                    }
                    if let Some(u) = m.get("usage") {
                        anthropic_usage(u, &mut call.usage);
                    }
                }
            }
            "content_block_start" => {
                let idx = e.get("index").and_then(|i| i.as_u64()).unwrap_or(blocks.len() as u64);
                let b = e.get("content_block").cloned().unwrap_or(Value::Null);
                if let Some(p) = anthropic_blocks(Some(&Value::Array(vec![b]))).pop() {
                    let p = match p {
                        Part::ToolCall(mut c) => {
                            // the input arrives as input_json_delta pieces
                            if c.arguments == "{}" {
                                c.arguments.clear();
                            }
                            Part::ToolCall(c)
                        }
                        p => p,
                    };
                    blocks.insert(idx, p);
                }
            }
            "content_block_delta" => {
                let idx = e.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                let d = e.get("delta").cloned().unwrap_or(Value::Null);
                match d.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => {
                        let t = d.get("text").map(s).unwrap_or_default();
                        if let Part::Text(a) = blocks.entry(idx).or_insert_with(|| Part::Text(String::new())) { a.push_str(&t) }
                    }
                    "thinking_delta" => {
                        let t = d.get("thinking").map(s).unwrap_or_default();
                        if let Part::Thinking(a) = blocks.entry(idx).or_insert_with(|| Part::Thinking(String::new())) {
                            a.push_str(&t);
                        }
                    }
                    "input_json_delta" => json_buf.entry(idx).or_default().push_str(&d.get("partial_json").map(s).unwrap_or_default()),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = e.get("delta").and_then(|d| d.get("stop_reason")).and_then(|r| r.as_str()) {
                    call.stop_reason = r.to_string();
                }
                if let Some(u) = e.get("usage") {
                    anthropic_usage(u, &mut call.usage);
                }
            }
            "error" => call.error = error_text(e),
            _ => {}
        }
    }
    for (idx, j) in json_buf {
        if let Some(Part::ToolCall(c)) = blocks.get_mut(&idx) {
            c.arguments = j;
        }
    }
    for p in blocks.values_mut() {
        if let Part::ToolCall(c) = p {
            if c.arguments.is_empty() {
                c.arguments = "{}".into();
            }
        }
    }
    call.reply = Message { role: "assistant".into(), parts: blocks.into_values().collect() };
}

/// Parses a captured session as an LLM API call, or None when it is not one.
pub fn parse(r: &SessionRecord) -> Option<LlmCall> {
    let kind = detect(r)?;
    let req: Value = serde_json::from_str(r.request_body.trim()).ok()?;
    let obj = req.as_object()?;
    let mut call = LlmCall {
        id: r.id,
        kind: Some(kind),
        provider: provider_name(r, kind),
        model: obj.get("model").map(s).unwrap_or_default(),
        status: r.status,
        ttft_ms: r.ttfb_ms,
        duration_ms: r.duration_ms,
        start_time: r.start_time.clone(),
        ..Default::default()
    };
    top_params(obj, &mut call.params);
    match kind {
        ApiKind::ChatCompletions | ApiKind::OllamaChat => parse_chat_request(obj, &mut call),
        ApiKind::Completions => call.messages.push(Message { role: "user".into(), parts: vec![Part::Text(obj.get("prompt").map(content_text).unwrap_or_default())] }),
        ApiKind::Responses => parse_responses_request(obj, &mut call),
        ApiKind::AnthropicMessages => parse_anthropic_request(obj, &mut call),
        ApiKind::Gemini => parse_gemini_request(r, obj, &mut call),
        ApiKind::OllamaGenerate => parse_ollama_generate_request(obj, &mut call),
    }
    let request_model = call.model.clone();
    call.streamed = is_streamed_body(r) || r.url.contains(":streamGenerateContent");
    let body = r.response_body.trim();
    if body.is_empty() {
        return Some(call);
    }
    if call.streamed {
        let events = stream_events(body);
        match kind {
            ApiKind::ChatCompletions | ApiKind::Completions => parse_chat_stream(&events, &mut call),
            ApiKind::Responses => parse_responses_stream(&events, &mut call),
            ApiKind::AnthropicMessages => parse_anthropic_stream(&events, &mut call),
            ApiKind::Gemini | ApiKind::OllamaChat | ApiKind::OllamaGenerate => {
                let mut parts = Vec::new();
                for e in &events {
                    if kind == ApiKind::Gemini {
                        parse_gemini_chunk(e, &mut call, &mut parts);
                    } else {
                        parse_ollama_chunk(e, &mut call, &mut parts);
                    }
                    if let Some(err) = error_text(e) {
                        call.error = Some(err);
                    }
                }
                call.reply = Message { role: "assistant".into(), parts };
            }
        }
    } else if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(err) = error_text(&v) {
            call.error = Some(err);
        }
        match kind {
            ApiKind::ChatCompletions | ApiKind::Completions => parse_chat_response(&v, &mut call),
            ApiKind::Responses => parse_responses_response(&v, &mut call),
            ApiKind::AnthropicMessages => parse_anthropic_response(&v, &mut call),
            ApiKind::Gemini | ApiKind::OllamaChat | ApiKind::OllamaGenerate => {
                let mut parts = Vec::new();
                if kind == ApiKind::Gemini {
                    parse_gemini_chunk(&v, &mut call, &mut parts);
                } else {
                    parse_ollama_chunk(&v, &mut call, &mut parts);
                }
                call.reply = Message { role: "assistant".into(), parts };
            }
        }
    } else if r.status >= 400 {
        call.error = Some(body.chars().take(300).collect());
    }
    if call.model.is_empty() {
        call.model = request_model;
    }
    if call.error.is_none() && r.status >= 400 {
        call.error = Some(format!("HTTP {}", r.status));
    }
    Some(call)
}

// ===================== Prices =====================

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ModelPrice {
    /// Matched case-insensitively as a substring of the model name; the longest matching pattern wins.
    pub pattern: String,
    /// USD per million input (prompt) tokens.
    pub input_per_m: f64,
    /// USD per million output tokens (thinking tokens are billed as output).
    pub output_per_m: f64,
    /// USD per million cached input tokens (prompt cache reads); None = same as input.
    #[serde(default)]
    pub cached_input_per_m: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PriceTable {
    #[serde(default)]
    pub note: String,
    pub models: Vec<ModelPrice>,
}

pub const PRICE_NOTE: &str = "EDITABLE ESTIMATES: rough public list prices in USD per 1M tokens, not authoritative. Prices change often and differ by contract, region, batch and long-context tiers - edit this file (or the AI > Price table window) to match what you actually pay.";

impl Default for PriceTable {
    fn default() -> Self {
        let p = |pattern: &str, i: f64, o: f64, c: Option<f64>| ModelPrice { pattern: pattern.into(), input_per_m: i, output_per_m: o, cached_input_per_m: c };
        PriceTable {
            note: PRICE_NOTE.into(),
            models: vec![
                p("gpt-5", 1.25, 10.0, Some(0.125)),
                p("gpt-5-mini", 0.25, 2.0, Some(0.025)),
                p("gpt-5-nano", 0.05, 0.40, Some(0.005)),
                p("gpt-4.1", 2.0, 8.0, Some(0.50)),
                p("gpt-4.1-mini", 0.40, 1.60, Some(0.10)),
                p("gpt-4.1-nano", 0.10, 0.40, Some(0.025)),
                p("gpt-4o", 2.50, 10.0, Some(1.25)),
                p("gpt-4o-mini", 0.15, 0.60, Some(0.075)),
                p("o3", 2.0, 8.0, Some(0.50)),
                p("o4-mini", 1.10, 4.40, Some(0.275)),
                p("claude-opus", 5.0, 25.0, Some(0.50)),
                p("claude-opus-4-1", 15.0, 75.0, Some(1.50)),
                p("claude-opus-4-0", 15.0, 75.0, Some(1.50)),
                p("claude-sonnet", 3.0, 15.0, Some(0.30)),
                p("claude-haiku", 1.0, 5.0, Some(0.10)),
                p("claude-3-5-haiku", 0.80, 4.0, Some(0.08)),
                p("gemini-2.5-pro", 1.25, 10.0, Some(0.31)),
                p("gemini-2.5-flash", 0.30, 2.50, Some(0.075)),
                p("gemini-2.5-flash-lite", 0.10, 0.40, Some(0.025)),
                p("gemini-2.0-flash", 0.10, 0.40, Some(0.025)),
                p("llama-3.3-70b", 0.59, 0.79, None),
                p("llama-3.1-8b", 0.05, 0.08, None),
                p("deepseek-chat", 0.27, 1.10, Some(0.07)),
                p("deepseek-reasoner", 0.55, 2.19, Some(0.14)),
                p("mistral-large", 2.0, 6.0, None),
            ],
        }
    }
}

/// A cost estimate split into its parts (USD).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cost {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
}

impl Cost {
    pub fn total(&self) -> f64 {
        self.input + self.cached + self.output
    }
}

impl PriceTable {
    pub fn default_path() -> PathBuf {
        std::env::var("FERROTAP_LLM_PRICES").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("llm_prices.json"))
    }

    /// Loads the table, or the defaults when the file is missing or unreadable.
    pub fn load(path: &Path) -> PriceTable {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let j = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, j).map_err(|e| e.to_string())
    }

    pub fn find(&self, model: &str) -> Option<&ModelPrice> {
        let m = model.to_ascii_lowercase();
        self.models.iter().filter(|p| !p.pattern.trim().is_empty() && m.contains(&p.pattern.trim().to_ascii_lowercase())).max_by_key(|p| p.pattern.trim().len())
    }

    /// Cost of a call; None when the model has no price (local models cost 0).
    pub fn cost(&self, call: &LlmCall) -> Option<Cost> {
        if is_local_provider(&call.provider) {
            return Some(Cost::default());
        }
        let p = self.find(&call.model)?;
        Some(cost_of(p, &call.usage))
    }
}

pub fn cost_of(p: &ModelPrice, u: &Usage) -> Cost {
    let cached = u.cached.min(u.input);
    let uncached = u.input - cached;
    Cost {
        input: uncached as f64 * p.input_per_m / 1e6,
        cached: cached as f64 * p.cached_input_per_m.unwrap_or(p.input_per_m) / 1e6,
        output: u.output as f64 * p.output_per_m / 1e6,
    }
}

// ===================== Totals and agent grouping =====================

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Totals {
    pub calls: usize,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub thinking: u64,
    pub cost: f64,
    /// Calls whose model has no price in the table.
    pub unpriced: usize,
    pub duration_ms: u128,
}

pub fn totals<'a>(calls: impl IntoIterator<Item = &'a LlmCall>, prices: &PriceTable) -> Totals {
    let mut t = Totals::default();
    for c in calls {
        t.calls += 1;
        t.input += c.usage.input;
        t.output += c.usage.output;
        t.cached += c.usage.cached;
        t.thinking += c.usage.thinking;
        t.duration_ms += c.duration_ms;
        match prices.cost(c) {
            Some(x) => t.cost += x.total(),
            None => t.unpriced += 1,
        }
    }
    t
}

/// Consecutive calls of one AI-agent conversation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentGroup {
    /// Session ids in step order.
    pub ids: Vec<usize>,
    pub provider: String,
    pub model: String,
}

fn conv_key(c: &LlmCall) -> String {
    format!("{}|{}", c.provider, c.system.trim())
}

/// True when `next` continues `prev`: same provider and system prompt, and next's message list starts with all of
/// prev's messages and is longer (the previous reply and any tool results were appended).
pub fn continues(prev: &LlmCall, next: &LlmCall) -> bool {
    if conv_key(prev) != conv_key(next) || next.messages.len() <= prev.messages.len() || prev.messages.is_empty() {
        return false;
    }
    prev.messages.iter().zip(next.messages.iter()).all(|(a, b)| a.signature() == b.signature())
}

/// Groups calls (in capture order) into agent conversations. Each call joins the most recent group whose last step
/// it continues; otherwise it starts a new group. Groups keep capture order of their first step.
pub fn group_agents(calls: &[LlmCall]) -> Vec<AgentGroup> {
    let mut order: Vec<&LlmCall> = calls.iter().collect();
    order.sort_by_key(|c| c.id);
    let mut groups: Vec<AgentGroup> = Vec::new();
    let mut last: Vec<&LlmCall> = Vec::new();
    for c in order {
        let found = (0..groups.len()).rev().find(|&g| continues(last[g], c));
        match found {
            Some(g) => {
                groups[g].ids.push(c.id);
                last[g] = c;
            }
            None => {
                groups.push(AgentGroup { ids: vec![c.id], provider: c.provider.clone(), model: c.model.clone() });
                last.push(c);
            }
        }
    }
    groups
}

// ===================== Response cache =====================

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CacheEntry {
    /// sha256 of the cache key (method + URL + normalized body).
    pub key: String,
    pub method: String,
    pub url: String,
    pub model: String,
    pub status: u16,
    pub response_headers: BTreeMap<String, String>,
    pub response_body: String,
    /// The captured session it came from.
    pub source_id: usize,
    pub added: String,
    #[serde(default)]
    pub hits: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct LlmCache {
    /// Serve identical LLM requests from the cache (the proxy hook).
    pub enabled: bool,
    /// Add every successful captured LLM call to the cache automatically.
    pub auto_add: bool,
    pub entries: Vec<CacheEntry>,
    #[serde(skip)]
    pub hits: u64,
    #[serde(skip)]
    pub misses: u64,
}

/// Request JSON with sorted keys and no whitespace; non-JSON bodies are only trimmed.
pub fn normalize_body(body: &str) -> String {
    canonical_json_str(body)
}

pub fn cache_key(method: &str, url: &str, body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(method.to_ascii_uppercase().as_bytes());
    h.update(b" ");
    h.update(url.trim().as_bytes());
    h.update(b"\n");
    h.update(normalize_body(body).as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Headers that no longer describe the stored (decoded) body.
fn replay_headers(h: &BTreeMap<String, String>, len: usize) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = h
        .iter()
        .filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "content-length" | "content-encoding" | "transfer-encoding" | "connection" | "keep-alive" | "date"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    out.push(("content-length".into(), len.to_string()));
    out.push(("x-ferrotap-llm-cache".into(), "hit".into()));
    out
}

impl LlmCache {
    pub fn default_path() -> PathBuf {
        std::env::var("FERROTAP_LLM_CACHE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("llm_cache.json"))
    }

    pub fn load(path: &Path) -> LlmCache {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let j = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, j).map_err(|e| e.to_string())
    }

    /// Adds a captured LLM session (successful responses only). Returns false when it is not cacheable
    /// or an identical request is already cached.
    pub fn add(&mut self, r: &SessionRecord) -> bool {
        if !(200..300).contains(&r.status) || r.response_body.is_empty() || r.response_header("x-ferrotap-llm-cache").is_some() {
            return false;
        }
        let Some(call) = parse(r) else { return false };
        if call.error.is_some() {
            return false;
        }
        let key = cache_key(&r.method, &r.url, &r.request_body);
        if self.entries.iter().any(|e| e.key == key) {
            return false;
        }
        self.entries.push(CacheEntry {
            key,
            method: r.method.clone(),
            url: r.url.clone(),
            model: call.model,
            status: r.status,
            response_headers: r.response_headers.clone(),
            response_body: r.response_body.clone(),
            source_id: r.id,
            added: chrono::Utc::now().to_rfc3339(),
            hits: 0,
        });
        true
    }

    /// Looks up an LLM request (counts a hit or a miss). Non-LLM requests are ignored (None, not counted).
    pub fn lookup(&mut self, method: &str, url: &str, body: &str) -> Option<&CacheEntry> {
        detect_parts(method, url, body)?;
        let key = cache_key(method, url, body);
        match self.entries.iter_mut().find(|e| e.key == key) {
            Some(e) => {
                e.hits += 1;
                self.hits += 1;
                Some(e)
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|e| e.key != key);
    }
}

/// The process-wide cache the proxy reads (loaded from `LlmCache::default_path()` on first use).
pub fn global_cache() -> &'static RwLock<LlmCache> {
    static G: OnceLock<RwLock<LlmCache>> = OnceLock::new();
    G.get_or_init(|| RwLock::new(LlmCache::load(&LlmCache::default_path())))
}

/// A cached response ready to send, plus the session record to log for it.
pub struct CacheHit {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub record: SessionRecord,
}

/// The proxy's request-path hook: when the LLM response cache is on and this request is an identical LLM call
/// already cached, returns the response to serve locally (the upstream API is not contacted).
pub fn cache_hook(method: &str, url: &str, request_headers: &BTreeMap<String, String>, request_body: &str, http_version: &str, start_time: &str) -> Option<CacheHit> {
    if !global_cache().read().ok()?.enabled {
        return None;
    }
    let mut g = global_cache().write().ok()?;
    let e = g.lookup(method, url, request_body)?.clone();
    drop(g);
    let headers = replay_headers(&e.response_headers, e.response_body.len());
    let record = SessionRecord {
        id: 0,
        method: method.to_string(),
        url: url.to_string(),
        status: e.status,
        http_version: http_version.to_string(),
        response_http_version: "HTTP/1.1".into(),
        request_headers: request_headers.clone(),
        response_headers: headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect(),
        request_body: request_body.to_string(),
        response_body: e.response_body.clone(),
        response_raw_size: Some(e.response_body.len()),
        response_decoded_size: Some(e.response_body.len()),
        start_time: start_time.to_string(),
        mark: Some(crate::model::SessionMark::Purple),
        comment: format!("LLM cache hit (from session #{})", e.source_id),
        ..Default::default()
    };
    Some(CacheHit { status: e.status, headers, body: e.response_body, record })
}

/// The ONE call site in proxy.rs (request path): builds the proxy response for a cache hit.
pub fn proxy_cache_response(method: &str, url: &str, request_headers: &BTreeMap<String, String>, request_body: &[u8], http_version: &str, start_time: &str) -> Option<(hudsucker::hyper::Response<hudsucker::Body>, SessionRecord)> {
    use http_body_util::Full;
    let body = String::from_utf8_lossy(request_body);
    let hit = cache_hook(method, url, request_headers, &body, http_version, start_time)?;
    let mut b = hudsucker::hyper::Response::builder().status(hit.status);
    for (k, v) in &hit.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    let resp = b.body(hudsucker::Body::from(Full::new(hudsucker::hyper::body::Bytes::from(hit.body.clone())))).ok()?;
    Some((resp, hit.record))
}

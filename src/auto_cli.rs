//! G16 headless / command-line mode: option parsing, per-session summary lines, writing the capture file.
//! The binary is `ferrotap_proxy` (src/main.rs).
use std::path::{Path, PathBuf};

use crate::model::{Rule, SessionRecord};

pub const USAGE: &str = "ferrotap_proxy - headless capture (no window)

USAGE: ferrotap_proxy [options]
  --port <n>          proxy port on 127.0.0.1 (default 8897)
  --out <file>        write captured sessions on exit: .har (HAR 1.2), .ftap, or anything else = JSON
  --every <secs>      also rewrite --out every N seconds
  --rules <file>      load Auto Responder rules (JSON array of rules, same format as rules.json)
  --scripts <file>    run scripts on every request / response: a .rhai file or a scripts.json list
                      (repeatable; plugins in the plugin folder are not loaded in headless mode)
  --duration <secs>   stop after N seconds (default: run until Ctrl+C)
  --quiet             no per-session lines
  --api-port <n>      enable the local REST API on 127.0.0.1:<n> (0 = pick a free port); off by default
  --api-token <t>     use this API token instead of a random one
  -h, --help          show this help";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CliOptions {
    pub port: u16,
    pub out: Option<PathBuf>,
    pub every: Option<u64>,
    pub rules: Option<PathBuf>,
    pub scripts: Vec<PathBuf>,
    pub duration: Option<u64>,
    pub quiet: bool,
    pub api_port: Option<u16>,
    pub api_token: Option<String>,
    pub help: bool,
}

impl CliOptions {
    /// Parse arguments (without the program name).
    pub fn parse<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> Result<CliOptions, String> {
        let mut o = CliOptions { port: crate::proxy::PORT, ..Default::default() };
        let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
                _ => (a, None),
            };
            let takes = matches!(name, "--port" | "--out" | "--every" | "--rules" | "--scripts" | "--duration" | "--api-port" | "--api-token");
            let val = if takes {
                match inline {
                    Some(v) => Some(v),
                    None => {
                        i += 1;
                        Some(args.get(i).cloned().ok_or_else(|| format!("{name} needs a value"))?)
                    }
                }
            } else {
                None
            };
            let num = |v: &Option<String>| -> Result<u64, String> {
                v.as_deref().unwrap_or("").parse::<u64>().map_err(|_| format!("{name}: not a number: {}", v.as_deref().unwrap_or("")))
            };
            match name {
                "--port" => o.port = u16::try_from(num(&val)?).map_err(|_| "--port out of range".to_string())?,
                "--out" => o.out = val.map(PathBuf::from),
                "--every" => o.every = Some(num(&val)?).filter(|n| *n > 0),
                "--rules" => o.rules = val.map(PathBuf::from),
                "--scripts" => o.scripts.extend(val.map(PathBuf::from)),
                "--duration" => o.duration = Some(num(&val)?),
                "--api-port" => o.api_port = Some(u16::try_from(num(&val)?).map_err(|_| "--api-port out of range".to_string())?),
                "--api-token" => o.api_token = val.filter(|t| !t.is_empty()),
                "--quiet" | "-q" => o.quiet = true,
                "--help" | "-h" => o.help = true,
                other => return Err(format!("unknown option: {other}")),
            }
            i += 1;
        }
        if o.every.is_some() && o.out.is_none() {
            return Err("--every needs --out".into());
        }
        if o.port == 0 {
            return Err("--port must not be 0".into());
        }
        Ok(o)
    }
}

/// One line per captured session.
pub fn summary_line(s: &SessionRecord) -> String {
    let size = s.response_body.len();
    let sz = if size >= 1024 * 1024 {
        format!("{:.1}MB", size as f64 / 1048576.0)
    } else if size >= 1024 {
        format!("{:.1}KB", size as f64 / 1024.0)
    } else {
        format!("{size}B")
    };
    let ct = s.content_type().split(';').next().unwrap_or("").trim().to_string();
    format!("#{} {} {} {} {}ms {} {}", s.id, s.method, s.status, s.url, s.duration_ms, sz, ct).trim_end().to_string()
}

/// Load a rules file (a JSON array of rules, or a single rule object).
pub fn load_rules_file(path: &Path) -> Result<Vec<Rule>, String> {
    let t = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))?;
    if v.is_array() {
        serde_json::from_value(v).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        serde_json::from_value::<Rule>(v).map(|r| vec![r]).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Write sessions to `path`; the format comes from the extension (.har, .ftap, else JSON array).
pub fn write_capture(path: &Path, sessions: &[SessionRecord]) -> Result<usize, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    match ext.as_str() {
        "har" => crate::har::export_har(path, sessions),
        "ftap" => crate::data_io::export_ftap(path, sessions),
        _ => {
            let j = serde_json::to_string_pretty(sessions).map_err(|e| e.to_string())?;
            std::fs::write(path, j).map_err(|e| format!("write {}: {e}", path.display()))?;
            Ok(sessions.len())
        }
    }
}

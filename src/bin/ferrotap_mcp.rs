//! ferrotap_mcp: MCP server over stdio that controls a running Ferrotap through its local REST API.
//!   ferrotap_mcp --api-port <n> --token <t>      (or env FERROTAP_API_PORT / FERROTAP_API_TOKEN)
//! Example MCP client entry: {"command": "ferrotap_mcp", "args": ["--api-port", "8898", "--token", "<token>"]}
use ferrotap::auto_mcp::{serve, HttpApi};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("ferrotap_mcp --api-port <n> --token <t>   (or env FERROTAP_API_PORT / FERROTAP_API_TOKEN)");
        return;
    }
    let port = val("--api-port")
        .or_else(|| std::env::var("FERROTAP_API_PORT").ok())
        .and_then(|p| p.parse::<u16>().ok());
    let token = val("--token").or_else(|| std::env::var("FERROTAP_API_TOKEN").ok()).unwrap_or_default();
    let Some(port) = port else {
        eprintln!("ferrotap_mcp: set --api-port or FERROTAP_API_PORT (enable the REST API in Ferrotap first)");
        std::process::exit(2);
    };
    let api = HttpApi { port, token };
    let stdin = std::io::stdin();
    if let Err(e) = serve(&api, stdin.lock(), std::io::stdout()) {
        eprintln!("ferrotap_mcp: {e}");
        std::process::exit(1);
    }
}

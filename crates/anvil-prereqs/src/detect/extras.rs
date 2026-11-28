//! Env file pairs and MCP server declarations.

use anvil_core::config::prereqs::EnvFilePrereq;
use serde_json::Value;

use super::Scan;

const EXAMPLES: &[&str] = &[".env.example", ".env.sample", ".env.template"];

pub(super) fn run(scan: &mut Scan<'_>) {
    env_pair(scan);
    mcp_servers(scan);
}

/// `.env.example` next to a (possibly missing) `.env`: the real file must define every key.
fn env_pair(scan: &mut Scan<'_>) {
    let Some(example) = EXAMPLES.iter().find(|f| scan.has(f)) else { return };
    let pair = EnvFilePrereq {
        file: scan.project.rel_file(".env"),
        example: scan.project.rel_file(example),
        subrepo: scan.subrepo(),
    };
    scan.out.env_files.push(pair);
}

fn mcp_servers(scan: &mut Scan<'_>) {
    let Some(json) = scan.read(".mcp.json").and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return;
    };
    let table = json.get("mcpServers").or_else(|| json.get("servers")).and_then(Value::as_object);
    for (name, def) in table.into_iter().flatten() {
        if !scan.out.mcp_servers.contains(name) {
            scan.out.mcp_servers.push(name.clone());
        }
        let text = def.to_string();
        if text.contains("chrome-devtools-mcp") {
            let source = scan.src(".mcp.json", &format!("server {name}"));
            scan.tool("chrome", None, source, false);
        }
        if text.contains("aws-api-mcp-server")
            || text.contains("awslabs.")
            || text.contains("awslabs-")
        {
            let source = scan.src(".mcp.json", &format!("server {name}"));
            scan.tool("aws", None, source, false);
        }
    }
}

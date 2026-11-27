//! MCP servers declared in `.mcp.json`: resolvable command, portable path, maintained package, no inline
//! credentials. Credentials are reported by scheme and host only.

use std::path::Path;

use serde_json::Value;

use super::Ctx;
use crate::creds::find_credentials;
use crate::envfile::read_text;
use crate::host::Host;
use crate::report::{Item, Kind, Status};

/// Servers of the reference repository that were archived in May 2025 and no longer get fixes.
const ARCHIVED: &[(&str, &str)] = &[
    (
        "@modelcontextprotocol/server-postgres",
        "use a maintained Postgres MCP server, e.g. crystaldba/postgres-mcp",
    ),
    ("@modelcontextprotocol/server-sqlite", "use a maintained SQLite MCP server"),
    ("mcp-server-sqlite", "use a maintained SQLite MCP server"),
    ("@modelcontextprotocol/server-puppeteer", "use @playwright/mcp or chrome-devtools-mcp"),
    ("@modelcontextprotocol/server-redis", "use the official redis/mcp-redis"),
    ("@modelcontextprotocol/server-github", "use GitHub's official github/github-mcp-server"),
    ("@modelcontextprotocol/server-gitlab", "use GitLab's own MCP server"),
    ("@modelcontextprotocol/server-slack", "use a maintained Slack MCP server"),
    ("@modelcontextprotocol/server-brave-search", "use Brave's official MCP server"),
    ("@modelcontextprotocol/server-google-maps", "use a maintained Google Maps MCP server"),
    ("@modelcontextprotocol/server-gdrive", "use a maintained Google Drive MCP server"),
    ("@modelcontextprotocol/server-everart", "use a maintained image-generation MCP server"),
    ("@modelcontextprotocol/server-sentry", "use Sentry's official MCP server"),
    ("@modelcontextprotocol/server-aws-kb-retrieval", "use the AWS Labs MCP servers"),
];

/// A server entry with the parts the checks look at.
struct Server {
    name: String,
    command: Option<String>,
    texts: Vec<String>,
}

pub(super) fn check_mcp(host: &dyn Host, ctx: &Ctx<'_>, wanted: &[String]) -> Vec<Item> {
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut items = Vec::new();
    for project in ctx.projects {
        let file = project.dir(ctx.root).join(".mcp.json");
        for server in read_servers(&file).into_iter().filter(|s| wanted.contains(&s.name)) {
            let source = project.rel_file(".mcp.json");
            items.push(check_server(host, &server, project.name.as_deref(), &source));
        }
    }
    items
}

fn read_servers(file: &Path) -> Vec<Server> {
    let Some(text) = read_text(file) else { return Vec::new() };
    let Ok(json) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    let table = json.get("mcpServers").or_else(|| json.get("servers")).and_then(Value::as_object);
    table.into_iter().flatten().map(|(name, def)| server_of(name, def)).collect()
}

fn server_of(name: &str, def: &Value) -> Server {
    let command = def.get("command").and_then(Value::as_str).map(str::to_owned);
    let mut texts: Vec<String> = command.iter().cloned().collect();
    let args = def.get("args").and_then(Value::as_array).into_iter().flatten();
    texts.extend(args.filter_map(Value::as_str).map(str::to_owned));
    let env = def.get("env").and_then(Value::as_object).into_iter().flatten();
    texts.extend(env.filter_map(|(_, v)| v.as_str()).map(str::to_owned));
    texts.extend(def.get("url").and_then(Value::as_str).map(str::to_owned));
    Server { name: name.to_owned(), command, texts }
}

fn check_server(host: &dyn Host, server: &Server, subrepo: Option<&str>, source: &str) -> Item {
    let mut item = Item::new(subrepo, Kind::Mcp, &server.name, Status::Ok).source(source);
    let mut findings: Vec<String> = Vec::new();
    let mut fixes: Vec<String> = Vec::new();
    if let Some(command) = &server.command {
        command_findings(host, command, &mut item, &mut findings, &mut fixes);
    }
    for text in &server.texts {
        for (package, fix) in ARCHIVED.iter().filter(|(p, _)| text.contains(p)) {
            raise(&mut item, Status::Warning);
            findings.push(format!("`{package}` is archived (2025-05) and no longer patched"));
            fixes.push((*fix).to_owned());
        }
    }
    let credentials: Vec<String> =
        server.texts.iter().flat_map(|t| find_credentials(t)).map(|c| c.label()).collect();
    if !credentials.is_empty() {
        raise(&mut item, Status::Warning);
        findings.push(format!("inline credentials in args or env ({})", credentials.join(", ")));
        fixes.push("pass the password through an environment variable instead".to_owned());
    }
    if findings.is_empty() {
        return item.message("command resolves");
    }
    item.hint = Some(fixes.join("; "));
    item.message(findings.join("; "))
}

fn command_findings(
    host: &dyn Host,
    command: &str,
    item: &mut Item,
    findings: &mut Vec<String>,
    fixes: &mut Vec<String>,
) {
    if is_absolute(command) {
        if let Some(owner) = foreign_owner(host, command) {
            raise(item, Status::Warning);
            findings.push(format!("absolute path inside another user's profile (`{owner}`)"));
            fixes.push("use a bare command name such as `npx` that resolves on PATH".to_owned());
        }
        if !Path::new(command).exists() {
            raise(item, Status::Missing);
            findings.push(format!("command `{command}` does not exist on this machine"));
            fixes.push("fix the path or use a bare command name".to_owned());
        }
    } else if host.which(command).is_none() {
        raise(item, Status::Missing);
        findings.push(format!("command `{command}` not found on PATH"));
        fixes.push(format!("install `{command}` or fix `command` in .mcp.json"));
    }
}

/// Keeps the worst status seen so far (`Missing` over `Warning` over `Ok`).
fn raise(item: &mut Item, to: Status) {
    if item.status == Status::Missing || (item.status == Status::Warning && to == Status::Warning) {
        return;
    }
    item.status = to;
}

fn is_absolute(command: &str) -> bool {
    let bytes = command.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || command.starts_with(['/', '\\'])
}

/// The profile owner when `command` lives under `C:\Users\<name>`, `/home/<name>` or `/Users/<name>`
/// and that is not the current user.
fn foreign_owner(host: &dyn Host, command: &str) -> Option<String> {
    let normal = command.replace('\\', "/");
    let lower = normal.to_ascii_lowercase();
    let rest = ["/users/", "/home/"].iter().find_map(|marker| {
        let at = lower.find(marker)?;
        (at <= 2).then(|| &normal[at + marker.len()..])
    })?;
    let owner = rest.split('/').next().filter(|o| !o.is_empty())?;
    let me = current_user(host)?;
    (!owner.eq_ignore_ascii_case(&me)).then(|| owner.to_owned())
}

fn current_user(host: &dyn Host) -> Option<String> {
    ["USERNAME", "USER", "LOGNAME"].iter().find_map(|v| host.env_var(v)).or_else(|| {
        let home = ["USERPROFILE", "HOME"].iter().find_map(|v| host.env_var(v))?;
        home.replace('\\', "/").rsplit('/').find(|s| !s.is_empty()).map(str::to_owned)
    })
}

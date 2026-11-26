//! `anvil plugin`: install the Claude Code mod for the whole user account through the `claude` CLI.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anvil_adapters::claude_code::plugin::{self, MARKETPLACE, PLUGIN_ID};
use anvil_core::fs_util::path::bin_in_path;
use anvil_core::proc_util::run_command_with_timeout;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::cli::subcmds::PluginSubcmd;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const CHANGE_TIMEOUT: Duration = Duration::from_secs(180);
const MISSING_CLAUDE: &str = "the `claude` CLI was not found on PATH; install Claude Code (https://claude.com/claude-code) and retry";

/// Executes `anvil plugin <subcommand>`.
pub fn execute(sub: PluginSubcmd) -> Result<()> {
    match sub {
        PluginSubcmd::Install => cmd_install(),
        PluginSubcmd::Uninstall => cmd_uninstall(),
        PluginSubcmd::Status { json } => cmd_status(json),
    }
}

struct Claude {
    bin: PathBuf,
}

impl Claude {
    fn locate() -> Result<Self> {
        bin_in_path("claude").map(|bin| Self { bin }).context(MISSING_CLAUDE)
    }

    fn run(&self, args: &[&OsStr], timeout: Duration) -> Result<String> {
        let shown = args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ");
        let out = run_command_with_timeout(Command::new(&self.bin).args(args), timeout)
            .with_context(|| format!("failed to run `claude {shown}`"))?;
        if out.timed_out {
            bail!("`claude {shown}` timed out after {}s", timeout.as_secs());
        }
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status_code != 0 {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("`claude {shown}` failed: {}", failure_line(&stdout, &stderr));
        }
        Ok(stdout)
    }

    fn call(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let os: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.run(&os, timeout)
    }

    fn list(&self, args: &[&str]) -> Result<Vec<Value>> {
        let stdout = self.call(args, QUERY_TIMEOUT)?;
        let parsed: Value =
            serde_json::from_str(stdout.trim()).context("`claude` printed invalid JSON")?;
        Ok(parsed.as_array().cloned().unwrap_or_default())
    }

    fn marketplace(&self) -> Result<Option<Source>> {
        let entries = self.list(&["plugin", "marketplace", "list", "--json"])?;
        Ok(entries.iter().find(|e| e["name"] == MARKETPLACE).map(Source::from_entry))
    }

    fn installed_version(&self) -> Result<Option<String>> {
        let entries = self.list(&["plugin", "list", "--json"])?;
        let found = entries.iter().find(|e| e["id"] == PLUGIN_ID && e["scope"] == "user");
        Ok(found.map(|e| e["version"].as_str().unwrap_or("unknown").to_owned()))
    }

    fn add_marketplace(&self, source: &OsStr) -> Result<()> {
        let args = ["plugin", "marketplace", "add"].map(OsStr::new);
        let mut all: Vec<&OsStr> = args.to_vec();
        all.extend([source, OsStr::new("--scope"), OsStr::new("user")]);
        self.run(&all, CHANGE_TIMEOUT).map(drop)
    }
}

fn failure_line(stdout: &str, stderr: &str) -> String {
    let text =
        [stderr, stdout].into_iter().map(str::trim).find(|t| !t.is_empty()).unwrap_or("no output");
    let message = serde_json::from_str::<Value>(text.lines().last().unwrap_or(text))
        .ok()
        .and_then(|v| v["error"].as_str().or(v["message"].as_str()).map(str::to_owned));
    message.unwrap_or_else(|| text.lines().next().unwrap_or(text).to_owned())
}

/// Where the `anvil` marketplace currently points.
struct Source {
    kind: String,
    location: String,
}

impl Source {
    fn from_entry(entry: &Value) -> Self {
        let kind = entry["source"].as_str().unwrap_or("unknown").to_owned();
        let location = ["path", "repo", "url"]
            .iter()
            .find_map(|key| entry[*key].as_str())
            .unwrap_or_default()
            .to_owned();
        Self { kind, location }
    }

    fn is_folder(&self, dir: &Path) -> bool {
        self.kind == "directory" && same_path(Path::new(&self.location), dir)
    }

    /// The argument that registers this source again, when `claude plugin marketplace add` accepts it.
    fn restore_arg(&self) -> Option<OsString> {
        matches!(self.kind.as_str(), "directory" | "github" | "git")
            .then(|| OsString::from(&self.location))
    }

    fn describe(&self) -> String {
        format!("{} {}", self.kind, self.location)
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

fn anvil_dir() -> Result<PathBuf> {
    let set = |name: &str| env::var_os(name).filter(|v| !v.is_empty());
    let dir = match set("ANVIL_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = set(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .context("cannot find the home folder; set ANVIL_HOME")?;
            PathBuf::from(home).join(".anvil")
        }
    };
    if dir.is_absolute() {
        return Ok(dir);
    }
    Ok(env::current_dir().context("failed to read the working directory")?.join(dir))
}

fn plugin_dir() -> Result<PathBuf> {
    Ok(anvil_dir()?.join("claude-plugin"))
}

fn cmd_install() -> Result<()> {
    let claude = Claude::locate()?;
    let dir = plugin_dir()?;
    let embedded = plugin::embedded_version()?;
    let previous = claude.marketplace()?;
    plugin::write_marketplace(&dir)?;
    println!("wrote the Anvil mod {embedded} to {}", dir.display());

    let displaced = previous.filter(|source| !source.is_folder(&dir));
    if let Some(old) = &displaced {
        println!(
            "marketplace `{MARKETPLACE}` points at {}; repointing it to {}",
            old.describe(),
            dir.display()
        );
    }
    claude.add_marketplace(dir.as_os_str())?;
    if let Err(err) = apply_plugin(&claude) {
        return Err(restore(&claude, displaced.as_ref(), err));
    }
    let installed =
        claude.installed_version()?.context("the plugin is not listed after installing it")?;
    println!("installed {PLUGIN_ID} {installed} (user scope)");
    if installed != embedded {
        println!("warning: this binary embeds {embedded}; Claude Code reports {installed}");
    }
    println!("Restart Claude Code sessions to load it.");
    Ok(())
}

fn apply_plugin(claude: &Claude) -> Result<()> {
    let verb = if claude.installed_version()?.is_some() { "update" } else { "install" };
    claude.call(&["plugin", verb, PLUGIN_ID, "--scope", "user"], CHANGE_TIMEOUT).map(drop)
}

fn restore(claude: &Claude, previous: Option<&Source>, cause: anyhow::Error) -> anyhow::Error {
    let Some(old) = previous else {
        return cause;
    };
    let restored = old.restore_arg().map(|arg| claude.add_marketplace(&arg));
    match restored {
        Some(Ok(())) => {
            cause.context(format!("restored the previous marketplace ({})", old.describe()))
        }
        _ => cause
            .context(format!("could not restore the previous marketplace ({})", old.describe())),
    }
}

fn cmd_uninstall() -> Result<()> {
    let claude = Claude::locate()?;
    let dir = plugin_dir()?;
    if claude.installed_version()?.is_some() {
        claude.call(&["plugin", "uninstall", PLUGIN_ID, "--scope", "user"], CHANGE_TIMEOUT)?;
        println!("uninstalled {PLUGIN_ID} (user scope)");
    }
    match claude.marketplace()? {
        Some(source) if source.is_folder(&dir) => {
            claude.call(
                &["plugin", "marketplace", "remove", MARKETPLACE, "--scope", "user"],
                CHANGE_TIMEOUT,
            )?;
            println!("removed marketplace `{MARKETPLACE}`");
        }
        Some(source) => {
            println!(
                "marketplace `{MARKETPLACE}` points at {}, not Anvil's folder; left in place",
                source.describe()
            );
        }
        None => {}
    }
    let removed = plugin::remove_marketplace(&dir)?;
    println!("removed {removed} files from {}", dir.display());
    println!("Restart Claude Code sessions to unload it.");
    Ok(())
}

fn cmd_status(as_json: bool) -> Result<()> {
    let embedded = plugin::embedded_version()?;
    let dir = plugin_dir()?;
    let claude = Claude::locate().ok();
    let installed = claude.as_ref().map(Claude::installed_version).transpose()?.flatten();
    let source = claude.as_ref().map(Claude::marketplace).transpose()?.flatten();
    if as_json {
        let marketplace = source.as_ref().map(|s| {
            json!({
                "source": s.kind,
                "location": s.location,
                "anvilFolder": s.is_folder(&dir),
            })
        });
        let report = json!({
            "claude": claude.is_some(),
            "embeddedVersion": embedded,
            "installedVersion": installed,
            "marketplace": marketplace,
            "folder": dir.display().to_string(),
        });
        println!("{report}");
        return Ok(());
    }
    println!("embedded version: {embedded}");
    println!("claude CLI:       {}", if claude.is_some() { "found" } else { "not found" });
    println!("installed:        {}", installed.as_deref().unwrap_or("no"));
    println!(
        "marketplace:      {}",
        source.as_ref().map_or_else(|| "none".to_owned(), Source::describe)
    );
    println!("anvil folder:     {}", dir.display());
    Ok(())
}

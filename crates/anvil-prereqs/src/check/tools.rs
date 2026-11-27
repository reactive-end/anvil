//! Tool checks: installed, version within the constraint, and the tools that are not a plain binary.

use std::path::Path;
use std::thread;

use anvil_core::config::prereqs::ToolPrereq;

use super::{Ctx, failure_status};
use crate::hints::install_hint;
use crate::host::{Host, HostOs};
use crate::probe::{Probe, canonical, probe, probe_java_home};
use crate::report::{Item, Kind, Status};
use crate::version::satisfies;

pub(super) fn check_tools(host: &dyn Host, ctx: &Ctx<'_>, tools: &[ToolPrereq]) -> Vec<Item> {
    thread::scope(|scope| {
        let handles: Vec<_> =
            tools.iter().map(|t| scope.spawn(move || check_tool(host, ctx, t))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
    })
}

fn check_tool(host: &dyn Host, ctx: &Ctx<'_>, tool: &ToolPrereq) -> Vec<Item> {
    let name = canonical(&tool.name);
    if matches!(name.as_str(), "xcodebuild" | "pod") && host.os() != HostOs::MacOs {
        return Vec::new();
    }
    let hint = if tool.hint.is_empty() {
        install_hint(&name, tool.version.as_deref())
    } else {
        tool.hint.clone()
    };
    let item = Item::new(tool.subrepo.as_deref(), Kind::Tool, &tool.name, Status::Ok)
        .constraint(tool.version.as_deref())
        .source(&tool.source)
        .required(tool.required)
        .hint(&hint);
    match name.as_str() {
        "anvil" => {
            let own = Probe::Found { version: env!("CARGO_PKG_VERSION").to_owned(), note: None };
            vec![judge(item, own)]
        }
        "android-sdk" => vec![android_sdk(host, &ctx.dir_of(tool.subrepo.as_deref()), item)],
        "chrome" => vec![chrome(host, item)],
        _ => {
            let found = probe(host, &name, ctx.opts.tool_timeout);
            let mut out = vec![judge(item.clone(), found)];
            if name == "java" {
                out.extend(java_home(host, ctx, tool, item));
            }
            out
        }
    }
}

/// Turns a probe into a status against the item's constraint.
fn judge(mut item: Item, found: Probe) -> Item {
    let required = item.required;
    match found {
        Probe::NotFound => {
            item.status = failure_status(required, Status::Missing);
            item.message("not found on PATH")
        }
        Probe::Unreadable(why) => {
            item.status = Status::Warning;
            item.message(format!("installed but its version is unreadable ({why})"))
        }
        Probe::Found { version, note } => {
            item.found = Some(version.clone());
            item.message = note;
            let Some(constraint) = item.constraint.clone() else { return item };
            match satisfies(&version, &constraint) {
                Ok(true) => {}
                Ok(false) => item.status = failure_status(required, Status::WrongVersion),
                Err(why) => {
                    item.status = Status::Warning;
                    item.message = Some(format!("cannot compare with `{constraint}`: {why}"));
                }
            }
            item
        }
    }
}

/// Gradle runs with `JAVA_HOME`, so it has to point at a JDK that satisfies the constraint too.
fn java_home(host: &dyn Host, ctx: &Ctx<'_>, tool: &ToolPrereq, base: Item) -> Option<Item> {
    let constraint = tool.version.as_deref()?;
    let mut item = base;
    item.name = "JAVA_HOME".to_owned();
    item.hint =
        Some(format!("Point JAVA_HOME at a JDK {constraint} (the JDK Gradle should run with)"));
    let Some(home) = host.env_var("JAVA_HOME") else {
        if !tool.source.contains("gradle") {
            return None;
        }
        item.status = Status::Warning;
        return Some(item.message("JAVA_HOME is not set"));
    };
    let found = probe_java_home(host, &home, ctx.opts.tool_timeout);
    let unusable = matches!(found, Probe::NotFound | Probe::Unreadable(_));
    let mut item = judge(item, found);
    if unusable {
        item.message = Some(format!("JAVA_HOME is `{home}` but it has no usable bin/java"));
    }
    Some(item)
}

fn android_sdk(host: &dyn Host, dir: &Path, mut item: Item) -> Item {
    let from_props = std::fs::read_to_string(dir.join("local.properties")).ok().and_then(|text| {
        text.lines().find_map(|l| l.trim().strip_prefix("sdk.dir=").map(unescape_properties))
    });
    let from_env = ["ANDROID_HOME", "ANDROID_SDK_ROOT"].iter().find_map(|v| host.env_var(v));
    let found = [from_props, from_env].into_iter().flatten().find(|p| Path::new(p).is_dir());
    match found {
        Some(path) => {
            item.found = Some(path);
            item
        }
        None => {
            item.status = failure_status(item.required, Status::Missing);
            item.message(
                "no SDK directory in local.properties sdk.dir, ANDROID_HOME or ANDROID_SDK_ROOT",
            )
        }
    }
}

/// `.properties` values escape `:` and `\`.
fn unescape_properties(value: &str) -> String {
    value.replace("\\:", ":").replace("\\\\", "\\")
}

const CHROME_BINARIES: &[&str] =
    &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"];
const CHROME_PATHS: &[&str] = &[
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];

fn chrome(host: &dyn Host, mut item: Item) -> Item {
    let from_env = host.env_var("CHROME_BIN").filter(|p| Path::new(p).is_file());
    let from_path =
        CHROME_BINARIES.iter().find_map(|n| host.which(n)).map(|p| p.display().to_string());
    let from_known = CHROME_PATHS.iter().find(|p| Path::new(p).is_file()).map(|p| (*p).to_owned());
    match from_env.or(from_path).or(from_known) {
        Some(path) => {
            item.found = Some(path);
            item
        }
        None => {
            item.status = failure_status(item.required, Status::Missing);
            item.message("no Chrome or Chromium found (CHROME_BIN, PATH, default install paths)")
        }
    }
}

//! The paths a shell command visibly writes, resolved against the directories it `cd`s into.

use std::path::{Component, Path, PathBuf};

use crate::commands::hook::shell::expand::{Invocation, expand};

use super::cmdlets::powershell_writes;

/// Lists the paths `cmd` visibly writes: redirections and operands of file-writing commands.
///
/// Relative paths are relative to the session's working directory: a `cd`, `pushd`, or
/// `Set-Location` with a literal argument earlier in the command line is applied and `.`/`..` are
/// resolved. Operands that hold an unexpanded variable, or follow a `cd` to an unknown directory,
/// cannot be checked statically and are left out.
pub fn written_paths(cmd: &str) -> Vec<String> {
    let mut dirs = WorkingDirs::default();
    let mut paths = Vec::new();
    for inv in &expand(cmd).invocations {
        let base = dirs.enter(inv.depth);
        paths.extend(invocation_writes(inv).iter().filter_map(|raw| resolve(base.as_deref(), raw)));
        if let Some(target) = change_directory(inv) {
            dirs.set(inv.depth, target.and_then(|dir| resolve(base.as_deref(), &dir)));
        }
    }
    paths
}

/// Removes `.` and `..` segments without touching the filesystem.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(out.components().next_back(), Some(Component::RootDir)) {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Redirect targets that discard output instead of writing a file.
pub fn is_discard(out: &str) -> bool {
    matches!(
        out.to_ascii_lowercase().as_str(),
        "/dev/null" | "nul" | "$null" | "1" | "2" | "/dev/stderr" | "/dev/stdout"
    )
}

/// The working directory of each nesting level; `None` once a `cd` target cannot be known.
struct WorkingDirs(Vec<Option<String>>);

impl Default for WorkingDirs {
    fn default() -> Self {
        Self(vec![Some(String::new())])
    }
}

impl WorkingDirs {
    /// The directory at `depth`. A nested script starts in its parent's directory, and what it
    /// changes does not outlive it.
    fn enter(&mut self, depth: usize) -> Option<String> {
        self.0.truncate(depth + 1);
        while self.0.len() <= depth {
            let inherited = self.0.last().cloned().flatten();
            self.0.push(inherited);
        }
        self.0[depth].clone()
    }

    fn set(&mut self, depth: usize, dir: Option<String>) {
        self.0[depth] = dir;
    }
}

/// `Some(target)` when `inv` changes directory; the target is `None` when it is not a literal.
fn change_directory(inv: &Invocation) -> Option<Option<String>> {
    let powershell = match inv.name.as_str() {
        "cd" | "chdir" | "pushd" => false,
        "set-location" | "sl" => true,
        "popd" => return Some(None),
        _ => return None,
    };
    let target = if powershell {
        powershell_writes(&inv.name, &inv.args).into_iter().next()
    } else {
        inv.args.iter().find(|a| !is_cd_option(a)).cloned()
    };
    Some(target.filter(|dir| dir != "-" && !dir.starts_with('~')))
}

fn is_cd_option(arg: &str) -> bool {
    matches!(arg, "-L" | "-P" | "-e" | "-@" | "--") || arg.eq_ignore_ascii_case("/d")
}

/// Joins `raw` onto `base` and normalises it; `None` when it cannot be known statically.
fn resolve(base: Option<&str>, raw: &str) -> Option<String> {
    if has_unexpanded_variable(raw) {
        return None;
    }
    let unified = raw.replace('\\', "/");
    if is_absolute(&unified) {
        return Some(normalize(&unified));
    }
    let base = base?;
    Some(normalize(&if base.is_empty() { unified } else { format!("{base}/{unified}") }))
}

fn is_absolute(unified: &str) -> bool {
    let bytes = unified.as_bytes();
    unified.starts_with('/')
        || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
}

fn normalize(unified: &str) -> String {
    let rooted = unified.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in unified.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part != ".." {
            parts.push(part);
            continue;
        }
        match parts.last() {
            Some(last) if last.ends_with(':') => {}
            Some(last) if *last != ".." => {
                parts.pop();
            }
            _ if rooted => {}
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// `$X`, `${X}`, `$(...)`, `$env:X`, and `%X%` are only known to the shell that runs the command.
fn has_unexpanded_variable(operand: &str) -> bool {
    let chars: Vec<char> = operand.chars().collect();
    chars.iter().enumerate().any(|(i, ch)| match ch {
        '$' => chars
            .get(i + 1)
            .is_some_and(|next| next.is_ascii_alphanumeric() || matches!(next, '_' | '{' | '(')),
        '%' => chars[i + 1..]
            .iter()
            .position(|c| *c == '%')
            .is_some_and(|end| end > 0 && chars[i + 1..i + 1 + end].iter().all(is_variable_char)),
        _ => false,
    })
}

fn is_variable_char(ch: &char) -> bool {
    ch.is_ascii_alphanumeric() || *ch == '_'
}

fn invocation_writes(inv: &Invocation) -> Vec<String> {
    let mut paths: Vec<String> =
        inv.outputs.iter().filter(|out| !is_discard(out)).cloned().collect();
    let operands = || inv.args.iter().filter(|a| !a.starts_with('-')).cloned();
    match inv.name.as_str() {
        "new-item" | "ni" | "set-content" | "add-content" | "ac" | "out-file" | "clear-content"
        | "clc" | "remove-item" | "ri" | "move-item" | "mi" | "rename-item" | "rni" | "ren"
        | "copy-item" | "cpi" => paths.extend(powershell_writes(&inv.name, &inv.args)),
        "rm" | "touch" | "tee" | "truncate" | "mkdir" | "rmdir" | "unlink" | "del" | "erase"
        | "mv" | "move" => paths.extend(operands()),
        "cp" | "copy" | "ln" => paths.extend(operands().next_back()),
        "install" => paths.extend(install_writes(&inv.args)),
        "sed" | "perl" if inv.args.iter().any(|a| a.starts_with("-i") || a == "--in-place") => {
            paths.extend(operands().skip(1));
        }
        _ => {}
    }
    paths
}

/// `install -d dir...` creates directories; otherwise the last of at least two operands is the
/// destination (`install -t dir` names it explicitly). `npx playwright install chromium` and
/// `npm install pkg` are not copies.
fn install_writes(args: &[String]) -> Vec<String> {
    if let Some(at) = args.iter().position(|a| a == "-t" || a == "--target-directory") {
        return args.get(at + 1).cloned().into_iter().collect();
    }
    let operands: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
    if args.iter().any(|a| a == "-d" || a == "--directory") {
        return operands;
    }
    if operands.len() < 2 {
        Vec::new()
    } else {
        operands.into_iter().next_back().into_iter().collect()
    }
}

#[cfg(test)]
mod tests;

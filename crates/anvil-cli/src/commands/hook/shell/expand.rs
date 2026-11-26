//! Flattens wrappers, nested shells, and substitutions into the invocations a command would run.

use super::command_name;
use super::lex::{Segment, parse};

const MAX_DEPTH: usize = 6;

const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "fish"];
const POWERSHELLS: &[&str] = &["powershell", "pwsh"];
const EVALUATORS: &[&str] = &["eval", "iex", "invoke-expression"];
const DOWNLOADERS: &[&str] =
    &["curl", "wget", "iwr", "irm", "invoke-webrequest", "invoke-restmethod", "base64"];
const KEYWORDS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!"];
const WRAPPERS: &[&str] = &[
    "sudo",
    "doas",
    "gsudo",
    "su",
    "env",
    "command",
    "builtin",
    "exec",
    "nohup",
    "time",
    "timeout",
    "gtimeout",
    "nice",
    "ionice",
    "stdbuf",
    "setsid",
    "xargs",
    "parallel",
    "busybox",
    "watch",
    "wsl",
    "ssh",
    "npx",
    "bunx",
    "script",
    "chroot",
    "start",
    "start-process",
    "runas",
    "strace",
];
const SCRIPT_ARG_WRAPPERS: &[&str] = &["su", "wsl", "ssh", "watch", "parallel", "script"];
const PRINTERS: &[&str] = &["echo", "printf", "write-output", "write-host"];
const SEARCHERS: &[&str] = &["rg", "grep", "egrep", "fgrep", "select-string", "sls", "findstr"];

/// A single program invocation reachable from the inspected command text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invocation {
    pub name: String,
    /// Arguments after the program name.
    pub args: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub stdin_text: Vec<String>,
    /// Operands arrive through a pipe or `xargs` and cannot be inspected.
    pub fed_by_pipe: bool,
    /// Output is consumed by a pipe or file instead of reaching the terminal.
    pub captured_out: bool,
}

/// Everything discovered while expanding a command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    pub invocations: Vec<Invocation>,
    /// Source passed inline to a language interpreter (`python -c`, `node -e`, heredocs).
    pub inline_code: Vec<String>,
    /// Reason the command cannot be inspected and needs human confirmation.
    pub opaque: Option<&'static str>,
}

/// Returns `true` for commands whose arguments are inert text rather than executed operands.
pub fn is_inert_text_command(name: &str, args: &[String]) -> bool {
    PRINTERS.contains(&name)
        || SEARCHERS.contains(&name)
        || (name == "git" && args.first().is_some_and(|sub| sub == "commit" || sub == "grep"))
}

/// Returns `true` for pattern-searching commands whose first operand is a pattern.
pub fn is_search_command(name: &str) -> bool {
    SEARCHERS.contains(&name)
}

/// Expands `cmd` into every invocation it would launch.
pub fn expand(cmd: &str) -> Expansion {
    let mut out = Expansion::default();
    expand_script(cmd, 0, &mut out);
    out
}

fn expand_script(cmd: &str, depth: usize, out: &mut Expansion) {
    if depth > MAX_DEPTH {
        out.opaque.get_or_insert("running deeply nested shell code that cannot be inspected");
        return;
    }
    let script = parse(cmd);
    for nested in &script.nested {
        expand_script(nested, depth + 1, out);
    }
    let mut previous: Option<&Segment> = None;
    for seg in &script.segments {
        let producer = previous.filter(|_| seg.piped_in);
        expand_segment(seg, producer, depth, out);
        previous = Some(seg);
    }
}

fn expand_segment(seg: &Segment, producer: Option<&Segment>, depth: usize, out: &mut Expansion) {
    let mut inv = Invocation {
        inputs: seg.inputs.clone(),
        outputs: seg.outputs.clone(),
        stdin_text: seg.stdin_text.clone(),
        fed_by_pipe: seg.piped_in,
        captured_out: seg.captured_out || depth > 0,
        ..Invocation::default()
    };
    if let Some(text) = producer.and_then(printed_text) {
        inv.stdin_text.push(text);
    }
    let Some((head, args)) = strip_prelude(&seg.args).split_first() else {
        out.invocations.push(inv);
        return;
    };
    inv.name = command_name(head);
    inv.args = args.to_vec();
    let downloaded = producer
        .and_then(|p| strip_prelude(&p.args).first())
        .is_some_and(|word| DOWNLOADERS.contains(&command_name(word).as_str()));
    if seg.piped_in && reads_program_from_stdin(&inv, downloaded) {
        out.opaque.get_or_insert("piping content into a shell or interpreter");
    }
    expand_invocation(inv, depth, out);
}

fn expand_invocation(inv: Invocation, depth: usize, out: &mut Expansion) {
    let name = inv.name.as_str();
    if SHELLS.contains(&name) {
        for text in &inv.stdin_text {
            expand_script(text, depth + 1, out);
        }
    }
    if is_encoded_powershell(&inv) {
        out.opaque.get_or_insert("running an encoded PowerShell command");
    }
    if let Some(script) = inline_script(&inv) {
        expand_script(&script, depth + 1, out);
    }
    if is_interpreter(name) {
        collect_inline_code(&inv, depth, out);
    }
    if is_wrapper(&inv) {
        expand_wrapped(&inv, depth, out);
    }
    out.invocations.push(inv);
}

/// Tries every non-flag suffix of a wrapper as the wrapped command, independent of flag arity.
fn expand_wrapped(wrapper: &Invocation, depth: usize, out: &mut Expansion) {
    let parses_script_args = SCRIPT_ARG_WRAPPERS.contains(&wrapper.name.as_str());
    for (idx, word) in wrapper.args.iter().enumerate() {
        if parses_script_args && word.contains(char::is_whitespace) {
            expand_script(word, depth + 1, out);
        }
        if word.starts_with('-') || is_env_assignment(word) {
            continue;
        }
        let inner = Invocation {
            name: command_name(word),
            args: wrapper.args[idx + 1..].to_vec(),
            inputs: wrapper.inputs.clone(),
            outputs: wrapper.outputs.clone(),
            stdin_text: wrapper.stdin_text.clone(),
            fed_by_pipe: wrapper.fed_by_pipe || wrapper.name == "xargs",
            captured_out: wrapper.captured_out || wrapper.name == "anvil",
        };
        if is_wrapper(&inner) {
            continue;
        }
        let inert = is_inert_text_command(&inner.name, &inner.args);
        expand_invocation(inner, depth, out);
        if inert {
            break;
        }
    }
}

fn is_wrapper(inv: &Invocation) -> bool {
    WRAPPERS.contains(&inv.name.as_str())
        || (inv.name == "anvil" && inv.args.iter().any(|a| a == "run"))
}

fn is_interpreter(name: &str) -> bool {
    matches!(name, "py" | "node" | "deno" | "bun" | "perl" | "ruby" | "php")
        || name.starts_with("python")
}

fn strip_prelude(words: &[String]) -> &[String] {
    let start = words
        .iter()
        .position(|w| !is_env_assignment(w) && !KEYWORDS.contains(&w.as_str()))
        .unwrap_or(words.len());
    &words[start..]
}

fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(key, _)| {
        key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn printed_text(seg: &Segment) -> Option<String> {
    let (head, args) = strip_prelude(&seg.args).split_first()?;
    PRINTERS.contains(&command_name(head).as_str()).then(|| args.join(" "))
}

fn reads_program_from_stdin(inv: &Invocation, downloaded: bool) -> bool {
    let name = inv.name.as_str();
    if EVALUATORS.contains(&name) {
        return inv.args.is_empty();
    }
    let explicit_stdin = inv.args.iter().any(|a| a == "-s" || a == "-");
    let bare = explicit_stdin || inv.args.iter().all(|a| a.starts_with('-'));
    if SHELLS.contains(&name) {
        return bare && shell_script_arg(&inv.args).is_none();
    }
    if POWERSHELLS.contains(&name) {
        return explicit_stdin || inv.args.is_empty();
    }
    downloaded && is_interpreter(name) && bare
}

fn inline_script(inv: &Invocation) -> Option<String> {
    let name = inv.name.as_str();
    if EVALUATORS.contains(&name) {
        return (!inv.args.is_empty()).then(|| inv.args.join(" "));
    }
    if SHELLS.contains(&name) {
        return shell_script_arg(&inv.args);
    }
    if name == "cmd" {
        let idx = inv
            .args
            .iter()
            .position(|a| matches!(a.to_ascii_lowercase().as_str(), "/c" | "//c" | "/k" | "//k"))?;
        return Some(rejoin(&inv.args[idx + 1..]));
    }
    if POWERSHELLS.contains(&name) {
        return powershell_script(&inv.args);
    }
    None
}

fn shell_script_arg(args: &[String]) -> Option<String> {
    let idx = args.iter().position(|a| {
        a.len() > 1
            && a.starts_with('-')
            && a[1..].chars().all(|c| c.is_ascii_alphabetic())
            && a.contains('c')
    })?;
    args.get(idx + 1).cloned()
}

fn powershell_script(args: &[String]) -> Option<String> {
    let mut idx = 0usize;
    while let Some(arg) = args.get(idx) {
        let lower = arg.to_ascii_lowercase();
        if lower.len() > 1 && "-command".starts_with(&lower) {
            return Some(rejoin(&args[idx + 1..]));
        }
        if lower.len() > 1 && "-file".starts_with(&lower) {
            return None;
        }
        if !lower.starts_with('-') {
            return Some(rejoin(&args[idx..]));
        }
        let takes_value = ["-ex", "-ep", "-w", "-v", "-in", "-o", "-config"]
            .iter()
            .any(|flag| lower.starts_with(flag));
        idx += if takes_value { 2 } else { 1 };
    }
    None
}

fn is_encoded_powershell(inv: &Invocation) -> bool {
    POWERSHELLS.contains(&inv.name.as_str())
        && inv.args.iter().any(|a| {
            let lower = a.to_ascii_lowercase();
            lower == "-ec" || (lower.len() > 1 && "-encodedcommand".starts_with(&lower))
        })
}

fn rejoin(words: &[String]) -> String {
    if let [only] = words {
        return only.clone();
    }
    words
        .iter()
        .map(|w| {
            if w.contains(char::is_whitespace) && !w.contains('\'') {
                format!("'{w}'")
            } else {
                w.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_inline_code(inv: &Invocation, depth: usize, out: &mut Expansion) {
    for (idx, arg) in inv.args.iter().enumerate() {
        let is_flag = matches!(
            arg.as_str(),
            "-c" | "-e" | "-E" | "-r" | "-p" | "-pe" | "-ne" | "--eval" | "--print" | "eval"
        );
        let Some(code) = inv.args.get(idx + 1).filter(|_| is_flag) else {
            continue;
        };
        if spawns_processes(code) {
            for literal in string_literals(code) {
                expand_script(&literal, depth + 1, out);
            }
        }
        out.inline_code.push(code.clone());
    }
    out.inline_code.extend(inv.stdin_text.iter().cloned());
}

fn spawns_processes(code: &str) -> bool {
    ["system(", "subprocess", "exec", "spawn", "popen", "Popen", "passthru("]
        .iter()
        .any(|marker| code.contains(marker))
}

/// Extracts the contents of quoted string literals from inline source code.
pub fn string_literals(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut quote: Option<char> = None;
    let mut cur = String::new();
    let mut escaped = false;
    for ch in code.chars() {
        let Some(open) = quote else {
            if matches!(ch, '"' | '\'' | '`') {
                quote = Some(ch);
            }
            continue;
        };
        if escaped {
            cur.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == open {
            quote = None;
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    out
}

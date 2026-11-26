//! Secret and credential file detection for file tools and shell invocations.

use super::destructive::split_git_subcommand;
use super::expand::{Invocation, is_search_command, string_literals};

const READ_DESCRIPTION: &str = "reading or copying a secret/credential file from shell";

const SAFE_ENV_TEMPLATES: &[&str] =
    &[".env.example", ".env.sample", ".env.template", ".env.dist", ".env.schema"];
const SECRET_NAMES: &[&str] = &[
    ".env",
    ".env*",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".git-credentials",
    "credentials.json",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
];
const SECRET_EXTENSIONS: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".jks", ".keystore"];
const SECRET_SUFFIX_PATHS: &[&str] = &[".aws/credentials", ".kube/config", ".docker/config.json"];

const READERS: &[&str] = &[
    "cat",
    "tac",
    "nl",
    "bat",
    "type",
    "get-content",
    "gc",
    "head",
    "tail",
    "less",
    "more",
    "sed",
    "awk",
    "gawk",
    "perl",
    "base64",
    "xxd",
    "od",
    "hexdump",
    "strings",
    "cut",
    "sort",
    "uniq",
    "jq",
    "yq",
    "paste",
    "diff",
    "cmp",
    "curl",
    "wget",
    "scp",
    "rsync",
    "zip",
    "tar",
    "gzip",
    "openssl",
    "dd",
    "import-csv",
    "source",
    ".",
];
const COPIERS: &[&str] = &["cp", "copy", "mv", "move", "copy-item", "cpi", "move-item", "mi", "ln"];
const EXPRESSION_FIRST: &[&str] = &["sed", "awk", "gawk", "jq", "yq", "perl"];
const CODE_READ_MARKERS: &[&str] = &[
    "open(",
    "read_text",
    "read_bytes",
    "readFile",
    "File.read",
    "IO.read",
    "file_get_contents",
    "readlines",
    "createReadStream",
];

/// Returns `true` when `path` names a file that usually holds credentials.
pub fn is_secret_file(path: &str) -> bool {
    let norm = path.trim().replace('\\', "/").to_ascii_lowercase();
    let name = norm.rsplit('/').next().unwrap_or(&norm);
    if SAFE_ENV_TEMPLATES.contains(&name) {
        return false;
    }
    SECRET_NAMES.contains(&name)
        || name.starts_with(".env.")
        || SECRET_EXTENSIONS
            .iter()
            .any(|ext| name.strip_suffix(ext).is_some_and(|stem| !stem.is_empty()))
        || SECRET_SUFFIX_PATHS.iter().any(|suffix| norm.ends_with(suffix))
}

/// Describes why `inv` exposes a secret file, or `None` when it does not.
pub fn describe(inv: &Invocation) -> Option<&'static str> {
    let exposes = inv.inputs.iter().any(|path| is_secret_file(path))
        || secret_operands(inv).iter().any(|op| names_secret(op));
    exposes.then_some(READ_DESCRIPTION)
}

/// Describes secret reads inside inline interpreter source.
pub fn describe_code(code: &str) -> Option<&'static str> {
    let reads = CODE_READ_MARKERS.iter().any(|marker| code.contains(marker));
    (reads && string_literals(code).iter().any(|lit| names_secret(lit))).then_some(READ_DESCRIPTION)
}

fn secret_operands(inv: &Invocation) -> Vec<&str> {
    let name = inv.name.as_str();
    if name == "git" {
        return match split_git_subcommand(&inv.args) {
            Some(("show" | "diff" | "cat-file" | "blame" | "grep" | "log", rest)) => {
                rest.iter().map(String::as_str).collect()
            }
            _ => Vec::new(),
        };
    }
    let operands = inv.args.iter().map(String::as_str).filter(|a| !a.starts_with('-'));
    if COPIERS.contains(&name) {
        let mut sources: Vec<&str> = operands.collect();
        if sources.len() > 1 {
            sources.pop();
        }
        return sources;
    }
    if is_search_command(name) || EXPRESSION_FIRST.contains(&name) {
        return operands.skip(1).collect();
    }
    if READERS.contains(&name) {
        return operands.collect();
    }
    Vec::new()
}

/// Accepts plain paths plus `@file`, `key=@file`, `if=file`, and `rev:path` operand forms.
fn names_secret(operand: &str) -> bool {
    if operand.contains(char::is_whitespace) || operand.contains("://") {
        return false;
    }
    let tail = operand.rsplit(['@', '=']).next().unwrap_or(operand);
    let rev_path = tail.rsplit(':').next().unwrap_or(tail);
    is_secret_file(tail) || is_secret_file(rev_path)
}

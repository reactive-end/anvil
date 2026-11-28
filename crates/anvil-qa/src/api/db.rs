//! Database access for scenarios through Google MCP Toolbox for Databases (`toolbox invoke`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anvil_core::config::db::{DbConfig, connection_env};
use anvil_core::proc_util::run_command_with_timeout;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Name of the managed tool `anvil db init` writes: it runs one SQL text and returns the rows.
pub const SELECT_TOOL: &str = "anvil_select";

/// Where to get the binary when it is missing.
pub const DOWNLOAD_HINT: &str = "https://github.com/googleapis/mcp-toolbox/releases";

const INVOKE_TIMEOUT: Duration = Duration::from_secs(30);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Words that never belong in a read-only query, wherever they appear outside literals.
const FORBIDDEN: [&str; 18] = [
    "insert", "update", "delete", "drop", "alter", "create", "truncate", "attach", "detach",
    "pragma", "vacuum", "grant", "revoke", "merge", "into", "call", "exec", "execute",
];

/// Runs a Toolbox tool and returns its rows. A trait so scenarios can be tested without a database.
pub trait DbClient {
    /// Invokes `tool` with `params` (a JSON object, or `null` for none).
    fn invoke(&self, tool: &str, params: &Value) -> Result<Vec<Value>>;
}

/// Runs `sql` through the managed [`SELECT_TOOL`] after checking that it only reads.
pub fn select(client: &dyn DbClient, sql: &str) -> Result<Vec<Value>> {
    ensure_read_only(sql)?;
    client.invoke(SELECT_TOOL, &json!({ "sql": sql }))
}

/// Refuses anything that is not one `SELECT`/`WITH` statement without write keywords.
pub fn ensure_read_only(sql: &str) -> Result<()> {
    let words = scan_words(sql)?;
    match words.first().map(String::as_str) {
        Some("select" | "with") => {}
        Some(other) => {
            bail!("only SELECT queries are allowed (the statement starts with `{other}`)")
        }
        None => bail!("the SQL text is empty"),
    }
    if let Some(word) = words.iter().find(|w| FORBIDDEN.contains(&w.as_str())) {
        bail!("`{word}` is not allowed: scenarios and `anvil db query` only read the database");
    }
    Ok(())
}

/// Lowercased words outside string literals, quoted identifiers, and comments. Fails when a second
/// statement follows a `;`.
fn scan_words(sql: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut ended = false;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '-' if chars.peek() == Some(&'-') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => skip_block_comment(&mut chars),
            _ if ended => bail!("only one statement is allowed (found more after a `;`)"),
            ';' => ended = true,
            '\'' | '"' | '`' => skip_quoted(&mut chars, c),
            c if c.is_alphanumeric() || c == '_' || c == '$' => {
                let mut word = c.to_string();
                while let Some(next) =
                    chars.next_if(|n| n.is_alphanumeric() || *n == '_' || *n == '$')
                {
                    word.push(next);
                }
                if word.starts_with(|w: char| w.is_alphabetic() || w == '_') {
                    words.push(word.to_lowercase());
                }
            }
            _ => {}
        }
    }
    Ok(words)
}

fn skip_block_comment(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let mut previous = ' ';
    for next in chars.by_ref() {
        if previous == '*' && next == '/' {
            return;
        }
        previous = next;
    }
}

/// Skips to the closing `quote`; a doubled quote inside is an escaped one.
fn skip_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, quote: char) {
    while let Some(next) = chars.next() {
        if next == quote {
            if chars.peek() == Some(&quote) {
                chars.next();
            } else {
                return;
            }
        }
    }
}

/// The real client: one `toolbox invoke` process per call, run from the repository root.
#[derive(Debug, Clone)]
pub struct ToolboxClient {
    program: PathBuf,
    config: PathBuf,
    cwd: PathBuf,
    timeout: Duration,
    /// Connection values for the tools file's `${DB_*}` placeholders (see `connection_env`).
    envs: Vec<(String, String)>,
}

impl ToolboxClient {
    /// Client for the binary and tools file named by `db`. Nothing is run until the first call.
    pub fn new(root: &Path, db: &DbConfig) -> Self {
        Self {
            program: db.toolbox_program(root),
            config: db.config_path(root),
            cwd: root.to_path_buf(),
            timeout: INVOKE_TIMEOUT,
            envs: connection_env(root, db),
        }
    }

    /// `toolbox --stdio --config <file>` with the connection values set: the MCP server agents
    /// talk to (`anvil db serve`), so credentials stay in the project's `.env`.
    pub fn stdio_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("--stdio").arg("--config").arg(&self.config).current_dir(&self.cwd);
        command.envs(self.envs.iter().map(|(k, v)| (k, v)));
        command
    }

    /// The binary as configured (a bare name or a resolved path), for messages.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The tools file.
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// First line of `toolbox --version`, or why the binary cannot run.
    pub fn version(&self) -> Result<String> {
        let mut command = Command::new(&self.program);
        command.arg("--version").current_dir(&self.cwd);
        let output = run_command_with_timeout(&mut command, VERSION_TIMEOUT)
            .with_context(|| self.spawn_hint())?;
        if output.status_code != 0 {
            bail!("`{} --version` exited with {}", self.program.display(), output.status_code);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().next().unwrap_or_default().trim().to_owned())
    }

    fn spawn_hint(&self) -> String {
        format!(
            "failed to run `{}` (is Toolbox installed? download it from {DOWNLOAD_HINT} and set `db.toolbox`)",
            self.program.display()
        )
    }
}

impl DbClient for ToolboxClient {
    fn invoke(&self, tool: &str, params: &Value) -> Result<Vec<Value>> {
        let mut command = Command::new(&self.program);
        command.arg("invoke").arg(tool);
        if params.as_object().is_some_and(|map| !map.is_empty()) {
            command.arg(params.to_string());
        }
        // ERROR keeps the startup log out of stdout, which must hold only the rows.
        command.arg("--config").arg(&self.config).args(["--log-level", "ERROR"]);
        command.current_dir(&self.cwd).envs(self.envs.iter().map(|(k, v)| (k, v)));
        let output = run_command_with_timeout(&mut command, self.timeout)
            .with_context(|| self.spawn_hint())?;
        if output.timed_out {
            bail!("toolbox did not answer within {}s (tool `{tool}`)", self.timeout.as_secs());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status_code != 0 {
            bail!("toolbox: {}", failure_message(&String::from_utf8_lossy(&output.stderr)));
        }
        parse_rows(&stdout)
    }
}

/// The `Error: ...` line Toolbox ends with plus the detail lines after it (a failed connection
/// lists one cause per address there), else the last line.
fn failure_message(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let start = lines.iter().rposition(|l| l.starts_with("Error:"));
    let Some(first) = start.or_else(|| lines.len().checked_sub(1)) else {
        return "exited with an error and no message".to_owned();
    };
    let message = lines[first..].join(" ");
    message.strip_prefix("Error:").unwrap_or(&message).trim().to_owned()
}

fn parse_rows(stdout: &str) -> Result<Vec<Value>> {
    let parsed: Value = serde_json::from_str(stdout.trim()).with_context(|| {
        format!("toolbox printed something that is not JSON: {}", preview(stdout))
    })?;
    Ok(match parsed {
        Value::Array(rows) => rows,
        Value::Null => Vec::new(),
        other => vec![other],
    })
}

fn preview(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default();
    line.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_are_read_only_and_everything_else_is_refused() {
        for ok in [
            "SELECT * FROM orders",
            "  select count(*) from orders;  ",
            "-- count them\nSELECT COUNT(*) FROM orders",
            "/* hi */ WITH t AS (SELECT 1) SELECT * FROM t",
            "SELECT replace(name, 'a', 'b'), updated_at FROM customers WHERE note = 'drop; delete'",
            "SELECT \"delete\" FROM t",
        ] {
            ensure_read_only(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "DELETE FROM orders",
            "INSERT INTO orders VALUES (1)",
            "PRAGMA table_info(orders)",
            "SELECT 1; DROP TABLE orders",
            "SELECT 1;\n-- fine\nDELETE FROM orders",
            "WITH t AS (SELECT 1) DELETE FROM orders",
            "SELECT * INTO copy FROM orders",
            "",
            "  -- only a comment",
        ] {
            assert!(ensure_read_only(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn the_refusal_names_the_problem() {
        let err = ensure_read_only("UPDATE orders SET status = 'x'").unwrap_err().to_string();
        assert!(err.contains("starts with `update`"), "{err}");
        let err = ensure_read_only("SELECT 1; SELECT 2").unwrap_err().to_string();
        assert!(err.contains("one statement"), "{err}");
    }

    #[test]
    fn toolbox_failures_surface_their_last_error_line() {
        let stderr = "2026-11-27T13:13:19-04:00 ERROR \"tool execution failed\"\nError: tool execution failed: no such table: nope (1)\n";
        assert_eq!(failure_message(stderr), "tool execution failed: no such table: nope (1)");
        assert_eq!(failure_message(""), "exited with an error and no message");
        let refused = "Error: unable to connect: failed to connect to `user=postgres`:\n\t[::1]:5432 (localhost): failed SASL auth (SQLSTATE 28P01)\n";
        assert_eq!(
            failure_message(refused),
            "unable to connect: failed to connect to `user=postgres`: [::1]:5432 (localhost): failed SASL auth (SQLSTATE 28P01)"
        );
    }

    #[test]
    fn rows_are_parsed_from_the_json_array() {
        let rows = parse_rows("[\n  {\"total\": 3}\n]\n").expect("rows");
        assert_eq!(rows, vec![json!({ "total": 3 })]);
        assert!(parse_rows("[]").expect("empty").is_empty());
        assert!(parse_rows("oops").is_err());
    }

    #[test]
    fn a_missing_binary_says_where_to_get_it() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = DbConfig { toolbox: "./no-such-toolbox".to_owned(), ..DbConfig::default() };
        let client = ToolboxClient::new(dir.path(), &db);
        let err =
            format!("{:#}", client.invoke(SELECT_TOOL, &json!({ "sql": "SELECT 1" })).unwrap_err());
        assert!(err.contains("is Toolbox installed"), "{err}");
        assert!(client.version().is_err());
    }
}

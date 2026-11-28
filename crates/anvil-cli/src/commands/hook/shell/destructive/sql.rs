//! Destructive SQL, MongoDB, and Redis statements in command arguments and database client input.

use super::super::command_name;
use super::super::expand::Invocation;

/// Statements dangerous in any command line, because the text rarely appears by accident.
const SQL_PATTERNS: &[(&str, &str)] = &[
    ("drop database", "destructive SQL (`DROP DATABASE`)"),
    ("drop schema", "destructive SQL (`DROP SCHEMA`)"),
    ("drop table", "destructive SQL (`DROP TABLE`)"),
    ("truncate table", "destructive SQL (`TRUNCATE TABLE`)"),
    ("drop index", "destructive SQL (`DROP INDEX`)"),
];
/// Mongo calls matched as raw text inside database clients and MCP database tools.
const NOSQL_PATTERNS: &[(&str, &str)] = &[
    ("dropdatabase(", "dropping a MongoDB database (`dropDatabase()`)"),
    (".drop(", "dropping a MongoDB collection (`.drop()`)"),
];
const REDIS_FLUSH: &[&str] = &["flushall", "flushdb"];
const SQL_CLIENTS: &[&str] = &[
    "psql",
    "mysql",
    "mariadb",
    "sqlite3",
    "sqlcmd",
    "sqlplus",
    "duckdb",
    "mongo",
    "mongosh",
    "redis-cli",
];

/// Scans the arguments of `inv`, plus its stdin when it is a database client.
pub(super) fn describe_invocation(inv: &Invocation) -> Option<&'static str> {
    let mut text = inv.args.join(" ");
    let in_client = SQL_CLIENTS.contains(&inv.name.as_str());
    if in_client {
        for stdin in &inv.stdin_text {
            text.push(' ');
            text.push_str(stdin);
        }
    }
    let wraps_client = inv.args.iter().any(|a| SQL_CLIENTS.contains(&command_name(a).as_str()));
    let flushes = matches!(inv.name.as_str(), "flushall" | "flushdb");
    describe_text(&text, in_client || wraps_client || flushes)
}

/// Scans SQL-like text; the broader statements run only where the text is known to reach a database.
pub(super) fn describe_text(text: &str, database_context: bool) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let sql = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((_, desc)) = SQL_PATTERNS.iter().find(|(pattern, _)| sql.contains(pattern)) {
        return Some(desc);
    }
    if !database_context {
        return None;
    }
    if let Some((_, desc)) = NOSQL_PATTERNS.iter().find(|(pattern, _)| sql.contains(pattern)) {
        return Some(desc);
    }
    describe_loose(&sql)
}

/// Statements that are only unambiguous once the text is known to be database input.
fn describe_loose(sql: &str) -> Option<&'static str> {
    let is_separator = |c: char| c.is_whitespace() || matches!(c, ';' | '(' | ')' | '"' | '\'');
    if sql.split(is_separator).any(|word| REDIS_FLUSH.contains(&word)) {
        return Some("flushing every key of a Redis server (`FLUSHALL` / `FLUSHDB`)");
    }
    sql.split(';').find_map(describe_statement)
}

fn describe_statement(statement: &str) -> Option<&'static str> {
    let words: Vec<&str> = statement.split(|c: char| c.is_whitespace() || c == '"').collect();
    let after = |keyword: &str| words.iter().position(|w| *w == keyword).map(|idx| idx + 1);
    if after("truncate").is_some_and(|next| words.get(next).is_some_and(|w| *w != "table")) {
        return Some("destructive SQL (`TRUNCATE`)");
    }
    let delete_from = words.windows(2).any(|pair| pair == ["delete", "from"]);
    if delete_from && !words.contains(&"where") {
        return Some("destructive SQL (`DELETE FROM` without `WHERE`)");
    }
    let alter = words.windows(2).position(|pair| pair == ["alter", "table"])?;
    words[alter..].contains(&"drop").then_some("destructive SQL (`ALTER TABLE ... DROP`)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_unfiltered_deletes_and_bare_truncates_in_database_context() {
        for sql in [
            "truncate users",
            "TRUNCATE users;",
            "delete from users",
            "DELETE FROM users;",
            "select 1; delete from users",
            "alter table users drop column email",
            "flushall",
            "FLUSHDB",
            "db.dropDatabase()",
            "db.users.drop()",
        ] {
            assert!(describe_text(sql, true).is_some(), "expected a risk for `{sql}`");
        }
        assert!(describe_text("drop index idx_users", false).is_some());
    }

    #[test]
    fn ignores_filtered_deletes_and_statements_outside_database_context() {
        for sql in [
            "delete from users where id = 1",
            "select * from users",
            "alter table users add column x int",
            "create index idx on users(id)",
        ] {
            let found = describe_text(sql, true);
            assert!(found.is_none(), "unexpected risk for `{sql}`: {found:?}");
        }
        assert!(describe_text("delete from users", false).is_none());
        assert!(describe_text("truncate users", false).is_none());
    }
}

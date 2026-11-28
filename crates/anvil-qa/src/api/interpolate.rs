//! `{{variable}}` substitution for scenario strings.
//!
//! A string that is exactly `{{name}}` takes the variable's JSON value with its type, so a captured
//! count stays a number. Anywhere else the value is written into the text. A variable that does not
//! exist is an error: a silently empty value would make a check pass or fail for the wrong reason.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::Value;

/// Scenario variables: the `vars` of the file, `$run`, and everything captured so far.
pub type Vars = BTreeMap<String, Value>;

/// Name of the built-in variable that is unique for every scenario execution.
pub const RUN_VAR: &str = "$run";

/// Interpolates every string inside `value` (object keys are left alone).
pub fn value(value: &Value, vars: &Vars) -> Result<Value> {
    Ok(match value {
        Value::String(text) => match exact_placeholder(text) {
            Some(name) => lookup(name, vars)?.clone(),
            None => Value::String(expand(text, vars, false)?),
        },
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| self::value(item, vars)).collect::<Result<_>>()?)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), self::value(item, vars)?)))
                .collect::<Result<_>>()?,
        ),
        other => other.clone(),
    })
}

/// Interpolates plain text (paths, headers, bodies); values are written as they are.
pub fn text(text: &str, vars: &Vars) -> Result<String> {
    expand(text, vars, false)
}

/// Interpolates SQL text: a `'` inside a substituted value is doubled so it cannot end the literal
/// the author wrote around the placeholder. Tool parameters are the safe form for untrusted data.
pub fn sql(text: &str, vars: &Vars) -> Result<String> {
    expand(text, vars, true)
}

fn lookup<'a>(name: &str, vars: &'a Vars) -> Result<&'a Value> {
    match vars.get(name) {
        Some(found) => Ok(found),
        None => {
            let known: Vec<&str> = vars.keys().map(String::as_str).collect();
            bail!("unknown variable `{name}` (known: {})", known.join(", "))
        }
    }
}

/// The variable name when the whole string is one placeholder.
fn exact_placeholder(text: &str) -> Option<&str> {
    let inner = text.trim().strip_prefix("{{")?.strip_suffix("}}")?;
    if inner.contains("{{") || inner.contains("}}") {
        return None;
    }
    Some(inner.trim())
}

fn render(found: &Value) -> String {
    match found {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn expand(text: &str, vars: &Vars, escape_quotes: bool) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else { break };
        let name = rest[start + 2..start + 2 + len].trim();
        let rendered = render(lookup(name, vars)?);
        out.push_str(&rest[..start]);
        out.push_str(&if escape_quotes { rendered.replace('\'', "''") } else { rendered });
        rest = &rest[start + 2 + len + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn vars() -> Vars {
        Vars::from([
            ("id".to_owned(), json!("c-1")),
            ("count".to_owned(), json!(3)),
            ("name".to_owned(), json!("O'Brien")),
            (RUN_VAR.to_owned(), json!("r42")),
        ])
    }

    #[test]
    fn an_exact_placeholder_keeps_the_type() {
        assert_eq!(value(&json!("{{count}}"), &vars()).expect("ok"), json!(3));
        assert_eq!(value(&json!(" {{ count }} "), &vars()).expect("ok"), json!(3));
        assert_eq!(value(&json!("n={{count}}"), &vars()).expect("ok"), json!("n=3"));
    }

    #[test]
    fn nested_values_are_interpolated_but_keys_are_not() {
        let input = json!({ "{{id}}": ["a-{{id}}", { "n": "{{count}}" }], "keep": 1 });
        let out = value(&input, &vars()).expect("ok");
        assert_eq!(out, json!({ "{{id}}": ["a-c-1", { "n": 3 }], "keep": 1 }));
    }

    #[test]
    fn text_without_a_closing_brace_pair_is_left_alone() {
        assert_eq!(text("a {{ b", &vars()).expect("ok"), "a {{ b");
        assert_eq!(text("{\"a\":{{count}}}", &vars()).expect("ok"), "{\"a\":3}");
    }

    #[test]
    fn unknown_variables_are_an_error_that_lists_the_known_ones() {
        let err = text("{{missing}}", &vars()).expect_err("unknown").to_string();
        assert!(err.contains("unknown variable `missing`"), "{err}");
        assert!(err.contains("count"), "{err}");
        assert!(value(&json!(["{{missing}}"]), &vars()).is_err());
    }

    #[test]
    fn sql_values_get_their_quotes_doubled() {
        let out = sql("SELECT * FROM c WHERE name = '{{name}}' AND id = '{{id}}'", &vars());
        assert_eq!(out.expect("ok"), "SELECT * FROM c WHERE name = 'O''Brien' AND id = 'c-1'");
        assert_eq!(text("{{name}}", &vars()).expect("ok"), "O'Brien");
    }
}

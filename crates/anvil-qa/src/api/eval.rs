//! Judging a step's answer against its expectations, and capturing values from it.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::Value;

use super::http::HttpResponse;
use super::spec::Expect;

/// Longest text of a failure preview.
const PREVIEW_CHARS: usize = 400;

/// Rows shown in a failure preview.
const PREVIEW_ROWS: usize = 5;

/// What went wrong in a step, with what the API or database actually returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub reasons: Vec<String>,
    /// A truncated view of the answer (body or first rows).
    pub actual: Option<Value>,
}

impl Mismatch {
    /// All reasons on one line.
    pub fn message(&self) -> String {
        self.reasons.join("; ")
    }
}

/// Exact JSON equality, except that two numbers are compared by value (`2` equals `2.0`).
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(i), Some(j)) => i == j,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| values_equal(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, p)| y.get(key).is_some_and(|q| values_equal(p, q)))
        }
        _ => a == b,
    }
}

/// The columns of `expected` that `actual` lacks or has with another value.
fn column_diffs(expected: &Value, actual: &Value) -> Vec<String> {
    let Some(wanted) = expected.as_object() else { return Vec::new() };
    wanted
        .iter()
        .filter_map(|(column, want)| match actual.get(column) {
            Some(got) if values_equal(want, got) => None,
            Some(got) => Some(format!("`{column}` expected {want} but got {got}")),
            None => Some(format!("`{column}` is missing (expected {want})")),
        })
        .collect()
}

/// A short, printable form of `value`: at most a few rows and a few hundred characters.
pub fn preview(value: &Value) -> Value {
    let shown = match value {
        Value::Array(items) if items.len() > PREVIEW_ROWS => {
            Value::Array(items[..PREVIEW_ROWS].to_vec())
        }
        other => other.clone(),
    };
    let text = shown.to_string();
    if text.chars().count() <= PREVIEW_CHARS {
        return shown;
    }
    let cut: String = text.chars().take(PREVIEW_CHARS).collect();
    Value::String(format!("{cut}..."))
}

/// A short, single-line form of `text`.
pub fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(PREVIEW_CHARS).collect();
    format!("{cut}...")
}

/// Checks an HTTP answer. Without `expect.status`, any status below 400 passes: a scenario that
/// forgot the status must not pass on a server error.
pub fn check_http(expect: &Expect, response: &HttpResponse) -> Option<Mismatch> {
    let mut reasons = Vec::new();
    match expect.status {
        Some(status) if status != response.status => {
            reasons.push(format!("expected status {status} but got {}", response.status));
        }
        None if response.status >= 400 => reasons.push(format!(
            "got status {}; add `expect.status` if that is intended",
            response.status
        )),
        _ => {}
    }
    reasons.extend(json_reasons(expect, response));
    if let Some(text) = expect.body_contains.as_deref().filter(|t| !response.body.contains(t)) {
        reasons.push(format!("body does not contain `{text}`"));
    }
    reasons.extend(header_reasons(expect, response));
    if reasons.is_empty() {
        return None;
    }
    let actual =
        response.json.as_ref().map_or_else(|| Value::String(snippet(&response.body)), preview);
    Some(Mismatch { reasons, actual: Some(actual) })
}

fn json_reasons(expect: &Expect, response: &HttpResponse) -> Vec<String> {
    if expect.json.is_empty() {
        return Vec::new();
    }
    let Some(json) = &response.json else {
        return vec!["expected a JSON body but the response is not JSON".to_owned()];
    };
    expect
        .json
        .iter()
        .filter_map(|(pointer, want)| match json.pointer(pointer) {
            Some(got) if values_equal(want, got) => None,
            Some(got) => Some(format!("`{pointer}` expected {want} but got {got}")),
            None => Some(format!("`{pointer}` is missing (expected {want})")),
        })
        .collect()
}

fn header_reasons(expect: &Expect, response: &HttpResponse) -> Vec<String> {
    expect
        .headers
        .iter()
        .filter_map(|(name, part)| match response.header(name) {
            Some(value) if value.contains(part.as_str()) => None,
            Some(value) => {
                Some(format!("header `{name}` is `{value}`, expected it to contain `{part}`"))
            }
            None => Some(format!("header `{name}` is missing")),
        })
        .collect()
}

/// Checks the rows a database step returned.
pub fn check_db(expect: &Expect, rows: &[Value]) -> Option<Mismatch> {
    let mut reasons = Vec::new();
    if let Some(count) = expect.rows.filter(|count| *count != rows.len()) {
        reasons.push(format!("expected {count} row(s) but got {}", rows.len()));
    }
    if let Some(want) = &expect.row {
        match rows.first() {
            Some(first) => reasons
                .extend(column_diffs(want, first).into_iter().map(|d| format!("first row: {d}"))),
            None => reasons.push("expected a first row but the query returned none".to_owned()),
        }
    }
    if let Some(want) = &expect.contains
        && !rows.iter().any(|row| column_diffs(want, row).is_empty())
    {
        reasons.push(format!("no row contains {want}"));
    }
    if reasons.is_empty() {
        return None;
    }
    Some(Mismatch { reasons, actual: Some(preview(&Value::Array(rows.to_vec()))) })
}

/// Reads the values named by `captures` (variable to JSON pointer) out of `source`.
pub fn capture_values(
    captures: &BTreeMap<String, String>,
    source: Option<&Value>,
    what: &str,
) -> Result<Vec<(String, Value)>> {
    let mut found = Vec::new();
    for (name, pointer) in captures {
        let Some(root) = source else {
            bail!("capture `{name}`: the {what} is not JSON");
        };
        match root.pointer(pointer) {
            Some(value) => found.push((name.clone(), value.clone())),
            None => bail!("capture `{name}`: `{pointer}` not found in the {what}"),
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![(
                "content-type".to_owned(),
                "application/json; charset=utf-8".to_owned(),
            )],
            body: body.to_owned(),
            json: serde_json::from_str(body).ok(),
            truncated: false,
        }
    }

    fn expect(raw: Value) -> Expect {
        serde_json::from_value(raw).expect("expect")
    }

    #[test]
    fn numbers_compare_by_value_everything_else_exactly() {
        assert!(values_equal(&json!(2), &json!(2.0)));
        assert!(values_equal(&json!({ "a": [1, 2.0] }), &json!({ "a": [1.0, 2] })));
        assert!(!values_equal(&json!("2"), &json!(2)));
        assert!(!values_equal(&json!({ "a": 1 }), &json!({ "a": 1, "b": 2 })));
    }

    #[test]
    fn http_expectations_report_every_mismatch_with_the_actual_values() {
        let answer = response(404, r#"{"statusCode":404,"message":"no customer"}"#);
        let wanted = expect(json!({
            "status": 201,
            "json": { "/message": "ok", "/id": "x" },
            "headers": { "Content-Type": "json", "X-Trace": "1" },
            "body_contains": "created",
        }));
        let mismatch = check_http(&wanted, &answer).expect("mismatch");
        let text = mismatch.message();
        assert!(text.contains("expected status 201 but got 404"), "{text}");
        assert!(text.contains("`/message` expected \"ok\" but got \"no customer\""), "{text}");
        assert!(text.contains("`/id` is missing"), "{text}");
        assert!(text.contains("body does not contain `created`"), "{text}");
        assert!(text.contains("header `X-Trace` is missing"), "{text}");
        assert!(!text.contains("Content-Type"), "{text}");
        assert!(mismatch.actual.is_some());
    }

    #[test]
    fn a_matching_answer_passes_and_a_missing_status_rejects_server_errors() {
        let ok = response(201, r#"{"quantity":2,"id":"o-1"}"#);
        let wanted = expect(json!({ "status": 201, "json": { "/quantity": 2.0 } }));
        assert_eq!(check_http(&wanted, &ok), None);
        assert_eq!(check_http(&Expect::default(), &ok), None);
        let broken = response(500, "boom");
        let text = check_http(&Expect::default(), &broken).expect("500").message();
        assert!(text.contains("got status 500"), "{text}");
    }

    #[test]
    fn db_expectations_check_count_first_row_and_any_row() {
        let rows = vec![json!({ "status": "created", "quantity": 2 }), json!({ "status": "paid" })];
        let good = expect(
            json!({ "rows": 2, "row": { "status": "created" }, "contains": { "status": "paid" } }),
        );
        assert_eq!(check_db(&good, &rows), None);
        let bad = expect(
            json!({ "rows": 1, "row": { "status": "paid", "total": 1 }, "contains": { "status": "shipped" } }),
        );
        let text = check_db(&bad, &rows).expect("mismatch").message();
        assert!(text.contains("expected 1 row(s) but got 2"), "{text}");
        assert!(
            text.contains("first row: `status` expected \"paid\" but got \"created\""),
            "{text}"
        );
        assert!(text.contains("first row: `total` is missing"), "{text}");
        assert!(text.contains("no row contains {\"status\":\"shipped\"}"), "{text}");
        let none = expect(json!({ "row": { "a": 1 } }));
        assert!(check_db(&none, &[]).expect("empty").message().contains("returned none"));
    }

    #[test]
    fn captures_follow_json_pointers_and_fail_loudly() {
        let wanted = BTreeMap::from([("total".to_owned(), "/0/total".to_owned())]);
        let rows = json!([{ "total": 3 }]);
        let found = capture_values(&wanted, Some(&rows), "rows").expect("capture");
        assert_eq!(found, vec![("total".to_owned(), json!(3))]);
        let missing = BTreeMap::from([("x".to_owned(), "/1/total".to_owned())]);
        let err = capture_values(&missing, Some(&rows), "rows").unwrap_err().to_string();
        assert!(err.contains("`/1/total` not found"), "{err}");
        assert!(capture_values(&wanted, None, "response body").is_err());
    }

    #[test]
    fn previews_are_short() {
        let many = Value::Array((0..20).map(|n| json!({ "n": n })).collect());
        assert_eq!(preview(&many).as_array().map(Vec::len), Some(PREVIEW_ROWS));
        let long = Value::String("x".repeat(2000));
        assert!(preview(&long).as_str().is_some_and(|t| t.len() < PREVIEW_CHARS + 10));
        assert!(snippet(&"a ".repeat(500)).ends_with("..."));
    }
}

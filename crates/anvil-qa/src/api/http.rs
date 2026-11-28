//! The HTTP calls of a scenario, made with `ureq` and recorded for the expectations.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Longest answer body kept; a bigger body is cut and is not parsed as JSON.
pub const BODY_READ_CAP: u64 = 1 << 20;

/// Time one request may take, connection included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What a request sends as its body.
#[derive(Debug, Clone)]
pub enum Payload {
    None,
    Json(Value),
    Text(String),
}

/// A fully interpolated request.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub payload: Payload,
}

/// The answer of the API: status, headers (names lowercased), and the body.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// The body parsed as JSON, when it was complete and valid.
    pub json: Option<Value>,
    /// `true` when the body was longer than [`BODY_READ_CAP`] and got cut.
    pub truncated: bool,
}

impl HttpResponse {
    /// Value of the first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let wanted = name.to_ascii_lowercase();
        self.headers.iter().find(|(key, _)| *key == wanted).map(|(_, value)| value.as_str())
    }
}

/// Joins `path` to the API base URL; an absolute `http(s)://` path is used as it is.
pub fn resolve_url(base: Option<&str>, path: &str) -> Result<String> {
    if path.starts_with("http://") || path.starts_with("https://") {
        return Ok(path.to_owned());
    }
    let Some(base) = base else {
        bail!("relative path `{path}` needs `qa.api.url` in .harness/config.json");
    };
    Ok(format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/')))
}

/// Sends `request`. Any HTTP status is an answer; only a connection problem is an error.
pub fn send(request: &Request) -> Result<HttpResponse> {
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).redirects(0).build();
    let mut call = agent.request(&request.method.to_ascii_uppercase(), &request.url);
    for (name, value) in &request.headers {
        call = call.set(name, value);
    }
    let outcome = match &request.payload {
        Payload::None => call.call(),
        Payload::Json(value) => {
            let has_type = request.headers.keys().any(|k| k.eq_ignore_ascii_case("content-type"));
            if !has_type {
                call = call.set("Content-Type", "application/json");
            }
            call.send_string(&value.to_string())
        }
        Payload::Text(text) => call.send_string(text),
    };
    let response = match outcome {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(err)) => {
            bail!("{} {} failed: {err}", request.method.to_ascii_uppercase(), request.url)
        }
    };
    read_response(response)
}

fn read_response(response: ureq::Response) -> Result<HttpResponse> {
    let status = response.status();
    let headers = response
        .headers_names()
        .into_iter()
        .filter_map(|name| {
            let value = response.header(&name)?.to_owned();
            Some((name.to_ascii_lowercase(), value))
        })
        .collect();
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(BODY_READ_CAP + 1)
        .read_to_end(&mut bytes)
        .context("failed to read the response body")?;
    let truncated = bytes.len() as u64 > BODY_READ_CAP;
    if truncated {
        bytes.truncate(usize::try_from(BODY_READ_CAP).unwrap_or(usize::MAX));
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let json = if truncated { None } else { serde_json::from_str(&body).ok() };
    Ok(HttpResponse { status, headers, body, json, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testkit::serve;

    fn request(method: &str, url: String, payload: Payload) -> Request {
        Request { method: method.to_owned(), url, headers: BTreeMap::new(), payload }
    }

    #[test]
    fn relative_paths_join_the_base_and_absolute_ones_pass_through() {
        let joined = resolve_url(Some("http://localhost:3100/"), "/orders/1").expect("url");
        assert_eq!(joined, "http://localhost:3100/orders/1");
        assert_eq!(resolve_url(None, "https://x.test/a").expect("abs"), "https://x.test/a");
        assert!(resolve_url(None, "/orders").is_err());
    }

    #[test]
    fn json_answers_are_parsed_and_error_statuses_are_still_answers() {
        let server = serve(vec![
            ("201 Created", r#"{"id":"o-1","quantity":2}"#),
            ("404 Not Found", r#"{"message":"missing"}"#),
        ]);
        let created = send(&request(
            "post",
            format!("{}/orders", server.url),
            Payload::Json(serde_json::json!({ "item": "pen" })),
        ))
        .expect("created");
        assert_eq!(created.status, 201);
        assert_eq!(created.json.as_ref().and_then(|j| j.get("id")), Some(&Value::from("o-1")));
        assert_eq!(created.header("Content-Type"), Some("application/json"));
        let missing = send(&request("GET", format!("{}/orders/x", server.url), Payload::None))
            .expect("missing");
        assert_eq!(missing.status, 404);
        let seen = server.requests();
        assert!(seen[0].starts_with("POST /orders HTTP/1.1"), "{}", seen[0]);
        assert!(seen[0].contains(r#"{"item":"pen"}"#), "{}", seen[0]);
        assert!(seen[1].starts_with("GET /orders/x "), "{}", seen[1]);
    }

    #[test]
    fn a_closed_port_is_an_error_that_names_the_request() {
        let err = send(&request("GET", "http://127.0.0.1:1/".to_owned(), Payload::None))
            .expect_err("closed")
            .to_string();
        assert!(err.contains("GET http://127.0.0.1:1/ failed"), "{err}");
    }
}

//! Canonical HTTP route path normalization for matching server `ExposesRoute`
//! and client `CallsRoute` edges across languages and frameworks.

/// Joins a controller/router prefix and a method subpath into a slash-prefixed path.
pub fn join_route_paths(prefix: &str, subpath: &str) -> String {
    let p = prefix.trim().trim_matches('/');
    let s = subpath.trim().trim_matches('/');
    match (p.is_empty(), s.is_empty()) {
        (true, true) => "/".to_owned(),
        (false, true) => format!("/{p}"),
        (true, false) => format!("/{s}"),
        (false, false) => format!("/{p}/{s}"),
    }
}

/// Normalizes an HTTP method and raw route/URL into `"route:<METHOD> <canonical_path>"`.
///
/// Strips origins (`http://...`, `https://...`), leading base-URL template interpolations
/// (`${this.apiUrl}`, `${API_URL}`), query strings (`?foo=bar`), and fragments (`#hash`).
/// Collapses slashes, drops trailing slashes (except root `/`), and maps path parameters
/// (`:id`, `{id}`, `${id}`, `${order.id}`, `<id>`) to `:param`.
pub fn normalize_route_target(method: &str, raw_path: &str) -> Option<String> {
    let canonical_method = normalize_http_method(method)?;
    let trimmed = raw_path.trim();
    if trimmed.is_empty() || trimmed.len() > 256 || trimmed.contains(['\n', '\r']) {
        return None;
    }

    let without_query = trimmed.split(['?', '#']).next().unwrap_or(trimmed).trim();
    let without_origin = strip_origin(without_query);
    let path_part = strip_leading_base_interpolation(without_origin)?;
    if path_part.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    for raw_seg in split_path_segments(path_part) {
        let seg = raw_seg.trim();
        if seg.is_empty() {
            continue;
        }
        if is_param_segment(seg) {
            segments.push(":param");
        } else if seg.contains(char::is_whitespace) {
            return None;
        } else {
            segments.push(seg);
        }
    }

    if segments.is_empty() {
        Some(format!("route:{canonical_method} /"))
    } else {
        Some(format!("route:{canonical_method} /{}", segments.join("/")))
    }
}

fn normalize_http_method(method: &str) -> Option<&'static str> {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => Some("GET"),
        "POST" => Some("POST"),
        "PUT" => Some("PUT"),
        "PATCH" => Some("PATCH"),
        "DELETE" => Some("DELETE"),
        "HEAD" => Some("HEAD"),
        "OPTIONS" => Some("OPTIONS"),
        "ANY" | "ALL" => Some("ANY"),
        _ => None,
    }
}

fn strip_origin(input: &str) -> &str {
    let rest = if input.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://")) {
        &input[7..]
    } else if input.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")) {
        &input[8..]
    } else {
        return input;
    };

    match rest.find('/') {
        Some(idx) => &rest[idx..],
        None => "",
    }
}

fn strip_leading_base_interpolation(input: &str) -> Option<&str> {
    if !input.starts_with("${") {
        return Some(input);
    }
    let mut depth = 0usize;
    let mut close_idx = None;
    for (idx, ch) in input.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close_idx = Some(idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close_idx?;
    let rest = &input[close + 1..];
    if rest.is_empty() {
        return None;
    }
    if rest.starts_with('/') { Some(rest) } else { Some(input) }
}

fn split_path_segments(path: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut brace_depth = 0usize;

    for (idx, ch) in path.char_indices() {
        match ch {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '/' if brace_depth == 0 => {
                parts.push(&path[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&path[start..]);
    parts
}

fn is_param_segment(seg: &str) -> bool {
    (seg.starts_with(':') && seg.len() > 1)
        || (seg.starts_with('{') && seg.ends_with('}') && seg.len() > 2)
        || (seg.starts_with("${") && seg.ends_with('}') && seg.len() > 3)
        || (seg.starts_with('<') && seg.ends_with('>') && seg.len() > 2)
}

//! JSONC comment and trailing-comma stripping utility.

/// Strips JSONC `//` and `/* */` comments and trailing commas outside string literals.
pub fn strip_jsonc(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut no_comments = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < chars.len() {
        let c = chars[i];
        if in_string {
            no_comments.push(c);
            update_string_state(c, &mut in_string, &mut escaped);
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            no_comments.push(c);
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
        } else {
            no_comments.push(c);
            i += 1;
        }
    }
    strip_trailing_commas(&no_comments)
}

fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            update_string_state(c, &mut in_string, &mut escaped);
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == ',' && matches!(chars[i + 1..].iter().find(|ch| !ch.is_whitespace()), Some('}' | ']'))
        {
            continue;
        }
        out.push(c);
    }
    out
}

fn update_string_state(c: char, in_string: &mut bool, escaped: &mut bool) {
    if *escaped {
        *escaped = false;
    } else if c == '\\' {
        *escaped = true;
    } else if c == '"' {
        *in_string = false;
    }
}

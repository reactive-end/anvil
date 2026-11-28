//! Small line-based edits of `index.md` and `log.md` bodies.

fn with_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') { text.to_owned() } else { format!("{text}\n") }
}

/// Position (line index) one past the last non-blank line of the section starting at `heading_at`.
fn section_tail(lines: &[&str], heading_at: usize) -> usize {
    let mut tail = heading_at + 1;
    for (idx, line) in lines.iter().enumerate().skip(heading_at + 1) {
        if line.starts_with('#') {
            break;
        }
        if !line.trim().is_empty() {
            tail = idx + 1;
        }
    }
    tail
}

/// Appends `entry` (one Markdown line) to the section `heading`, creating the section when missing.
pub fn append_to_section(body: &str, heading: &str, entry: &str) -> String {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let Some(at) = lines.iter().position(|l| l.trim_end() == heading) else {
        return format!("{}\n{heading}\n\n{entry}\n", with_newline(body));
    };
    let tail = section_tail(&lines, at);
    let mut out: String = lines[..tail].concat();
    out = with_newline(&out);
    if tail == at + 1 {
        out.push('\n');
    }
    out.push_str(entry);
    out.push('\n');
    out.push_str(&lines[tail..].concat());
    out
}

/// Records `entry` under the `## <date>` heading, newest date first, creating the heading when needed.
pub fn add_log_entry(body: &str, date: &str, entry: &str) -> String {
    let heading = format!("## {date}");
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    if lines.iter().any(|l| l.trim_end() == heading) {
        return append_to_section(body, &heading, &format!("- {entry}"));
    }
    let fresh = format!("{heading}\n\n- {entry}\n\n");
    match lines.iter().position(|l| l.starts_with("## ")) {
        Some(first) => format!("{}{fresh}{}", lines[..first].concat(), lines[first..].concat()),
        None => format!("{}\n{}\n", with_newline(body), fresh.trim_end()),
    }
}

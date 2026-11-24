//! Terminal styling, TTY/NO_COLOR detection, structured cards, and HD 3D Braille Anvil banner.

pub mod file_picker;
pub mod prompt;

use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};

const COLOR_AUTO: u8 = 0;
const COLOR_ALWAYS: u8 = 1;
const COLOR_NEVER: u8 = 2;

static COLOR_MODE: AtomicU8 = AtomicU8::new(COLOR_AUTO);

/// Raytraced 3D isometric London-pattern anvil in HD Unicode Braille (76x18 -> 152x72 subpixels).
const ANVIL_ART_LINES: [&str; 18] = [
    "                                                  ⣀⣀⣀⣀⣠⣤⢤⣤⢴⣶⢶⣶⢶⣿⢿⣿⢿⣷⢄",
    "                                   ⢀⣀⣀⣀⣤⢤⣤⢤⣶⢶⣶⢾⣿⢿⣿⢿⣿⢿⣿⢿⡿⠿⠿⠽⣿⢿⣿⢟⣛⢽⣿⢿⣿⢿⣷⢦⡀",
    "                              ⢰⢶⣿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢶⣶⢿⣿⣿⣿⣿⣿⣿⣿⠿⢿⠟⢟⠟⢿",
    "                         ⢀⣀⣀⣀⣤⢼⢅⠝⠿⣿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⣿⣿⣿⣿⣿⣿⡿⢿⠿⢿⠟⢟⠟⢍⠝⠍⠕⢅⠑⢅⠕⢅⠅⢅⠕⢼",
    "                        ⢸⠝⢿⣿⢿⣿⢿⣵⢕⢅⠝⠿⣿⣿⠿⢿⠿⢟⠟⢟⠟⢍⠝⠍⠝⢅⠕⢅⠕⢅⠅⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⣕⠥⠓⠉",
    "                  ⢀⣀⣀⣤⣤⣶⣾⣿⢷⣝⢿⣿⣿⣿⣿⣵⣵⠅⠗⢅⠕⢅⠅⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⣵⠥⠓⠉",
    "         ⢀⣀⣠⢤⢤⢶⢶⢾⢽⢿⢽⢿⢽⢿⢽⢿⢽⢽⢽⠗⣿⠝⢍⠝⢅⠕⠅⡕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⢕⢅⠕⢅⡵⠕⠋⠁",
    "     ⠐⠒⠛⠛⠻⠿⠿⠿⠿⠽⠭⠽⠥⠽⠥⢝⣁⣙⣅⣙⣅⠙⠅⠕⣏⠕⢅⠕⠅⠕⠅⠗⢅⠕⢅⠕⢅⢕⢅⢕⢅⢕⢕⢕⢕⢕⢕⠕⢕⠕⣕⣽⠵⠋⠁",
    "                          ⠑⢄⠅⣷⠕⢅⢕⢵⢕⢿⢗⢕⠕⢕⠝⠕⠝⠕⠝⠕⠑⠅⠑⠅⠁⠁⠁⡠⠓⠁",
    "                            ⠑⠿⠕⠛⢻⠇⠁⠅⠁⠁⠁ ⠁ ⢁⠤⠥⠤⡁⠄⠁⠄⠅⠄⢥⢿⢿⢵⢤⢤⡀",
    "                           ⢀⣀⣀⣀⢤⢞⢕⢕⠄⡅⠄⠅⢅⠅⢅⡏   ⡗⢅⢕⢕⢕⢕⢟⣽⢽⢽⢿⢵⢽⢦⡀",
    "                       ⢸⢿⣽⢽⢽⢽⢽⢽⢽⢽⢽⢝⢽⣗⢕⢕⢕⢕⢕⢷⢤⢴⢖⢕⢕⢕⢵⢝⢵⢝⢵⢝⢽⢽⢽⢿⢽⣿⣦⡀",
    "                    ⡾⣽⢝⢕⢅⠝⢿⣽⢽⢽⢽⢽⢽⢽⢽⣽⢝⢵⢝⢵⢽⢵⢽⢵⢝⣽⣝⣽⢽⣽⡽⢽⠿⠿⠟⠟⠟⠏⠝⠍⣝⣽⣝⢶⣄",
    "                    ⡗⢍⠿⣽⢝⢕⢥⡍⠿⢽⣽⣽⣽⣽⣽⣽⠽⢽⠿⠟⠟⠟⠟⠍⠝⢍⣝⣅⣕⣥⣵⣥⣵⢷⣷⣵⡿⢿⠿⢿⠟⢟⠟⢟⢽",
    "                    ⠑⢅⠕⠭⡿⣽⢝⢝⢧⡝⠝⣍⣕⣅⣵⣥⣵⣥⣵⣷⣷⣷⠿⢿⠟⠋⠉⠙⢝⠏⠝⢍⠝⢅⠕⢅⠕⢅⠕⢅⠕⢅⢕⣅⣽",
    "                      ⠉⠉⢳⢍⠝⣿⠿⢿⠿⢝⠟⢟⠟⢍⠝⢍⠕⢅⠕⢅⡕⠁     ⠑⢵⣥⠵⠥⠵⠕⠓⠓⠓⠉⠉⠉⠉",
    "                        ⠈⠣⡕⣿⠕⢅⠕⢅⠕⢅⣕⣅⣕⡥⠵⠥⠵⠛",
    "                          ⠈⠛⠓⠓⠉⠉⠉⠁",
];

/// Plain 3D Braille Anvil banner used in `clap` `--help`.
pub const PLAIN_ANVIL_BANNER: &str = concat!(
    "                                                  ⣀⣀⣀⣀⣠⣤⢤⣤⢴⣶⢶⣶⢶⣿⢿⣿⢿⣷⢄\n",
    "                                   ⢀⣀⣀⣀⣤⢤⣤⢤⣶⢶⣶⢾⣿⢿⣿⢿⣿⢿⣿⢿⡿⠿⠿⠽⣿⢿⣿⢟⣛⢽⣿⢿⣿⢿⣷⢦⡀\n",
    "                              ⢰⢶⣿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢶⣶⢿⣿⣿⣿⣿⣿⣿⣿⠿⢿⠟⢟⠟⢿\n",
    "                         ⢀⣀⣀⣀⣤⢼⢅⠝⠿⣿⣿⢿⣿⢿⣿⢿⣿⢿⣿⢿⣿⣿⣿⣿⣿⣿⣿⡿⢿⠿⢿⠟⢟⠟⢍⠝⠍⠕⢅⠑⢅⠕⢅⠅⢅⠕⢼\n",
    "                        ⢸⠝⢿⣿⢿⣿⢿⣵⢕⢅⠝⠿⣿⣿⠿⢿⠿⢟⠟⢟⠟⢍⠝⠍⠝⢅⠕⢅⠕⢅⠅⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⣕⠥⠓⠉\n",
    "                  ⢀⣀⣀⣤⣤⣶⣾⣿⢷⣝⢿⣿⣿⣿⣿⣵⣵⠅⠗⢅⠕⢅⠅⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⣵⠥⠓⠉\n",
    "         ⢀⣀⣠⢤⢤⢶⢶⢾⢽⢿⢽⢿⢽⢿⢽⢿⢽⢽⢽⠗⣿⠝⢍⠝⢅⠕⠅⡕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⠕⢅⢕⢅⠕⢅⡵⠕⠋⠁\n",
    "     ⠐⠒⠛⠛⠻⠿⠿⠿⠿⠽⠭⠽⠥⠽⠥⢝⣁⣙⣅⣙⣅⠙⠅⠕⣏⠕⢅⠕⠅⠕⠅⠗⢅⠕⢅⠕⢅⢕⢅⢕⢅⢕⢕⢕⢕⢕⢕⠕⢕⠕⣕⣽⠵⠋⠁\n",
    "                          ⠑⢄⠅⣷⠕⢅⢕⢵⢕⢿⢗⢕⠕⢕⠝⠕⠝⠕⠝⠕⠑⠅⠑⠅⠁⠁⠁⡠⠓⠁\n",
    "                            ⠑⠿⠕⠛⢻⠇⠁⠅⠁⠁⠁ ⠁ ⢁⠤⠥⠤⡁⠄⠁⠄⠅⠄⢥⢿⢿⢵⢤⢤⡀\n",
    "                           ⢀⣀⣀⣀⢤⢞⢕⢕⠄⡅⠄⠅⢅⠅⢅⡏   ⡗⢅⢕⢕⢕⢕⢟⣽⢽⢽⢿⢵⢽⢦⡀\n",
    "                       ⢸⢿⣽⢽⢽⢽⢽⢽⢽⢽⢽⢝⢽⣗⢕⢕⢕⢕⢕⢷⢤⢴⢖⢕⢕⢕⢵⢝⢵⢝⢵⢝⢽⢽⢽⢿⢽⣿⣦⡀\n",
    "                    ⡾⣽⢝⢕⢅⠝⢿⣽⢽⢽⢽⢽⢽⢽⢽⣽⢝⢵⢝⢵⢽⢵⢽⢵⢝⣽⣝⣽⢽⣽⡽⢽⠿⠿⠟⠟⠟⠏⠝⠍⣝⣽⣝⢶⣄\n",
    "                    ⡗⢍⠿⣽⢝⢕⢥⡍⠿⢽⣽⣽⣽⣽⣽⣽⠽⢽⠿⠟⠟⠟⠟⠍⠝⢍⣝⣅⣕⣥⣵⣥⣵⢷⣷⣵⡿⢿⠿⢿⠟⢟⠟⢟⢽\n",
    "                    ⠑⢅⠕⠭⡿⣽⢝⢝⢧⡝⠝⣍⣕⣅⣵⣥⣵⣥⣵⣷⣷⣷⠿⢿⠟⠋⠉⠙⢝⠏⠝⢍⠝⢅⠕⢅⠕⢅⠕⢅⠕⢅⢕⣅⣽\n",
    "                      ⠉⠉⢳⢍⠝⣿⠿⢿⠿⢝⠟⢟⠟⢍⠝⢍⠕⢅⠕⢅⡕⠁     ⠑⢵⣥⠵⠥⠵⠕⠓⠓⠓⠉⠉⠉⠉\n",
    "                        ⠈⠣⡕⣿⠕⢅⠕⢅⠕⢅⣕⣅⣕⡥⠵⠥⠵⠛\n",
    "                          ⠈⠛⠓⠓⠉⠉⠉⠁\n\n",
    "  ◆  A N V I L  —  Native AI Engineering Harness  (AST · Code Graph · Memory · Tiered QA)"
);

/// Configures global color mode from CLI flags (`--no-color`) and env vars (`ANVIL_COLOR`, `NO_COLOR`, `CLICOLOR_FORCE`).
pub fn init_color_mode(no_color: bool) {
    if no_color {
        COLOR_MODE.store(COLOR_NEVER, Ordering::Relaxed);
        return;
    }
    if let Ok(val) = std::env::var("ANVIL_COLOR") {
        match val.trim().to_ascii_lowercase().as_str() {
            "always" | "1" | "true" => {
                COLOR_MODE.store(COLOR_ALWAYS, Ordering::Relaxed);
                return;
            }
            "never" | "0" | "false" => {
                COLOR_MODE.store(COLOR_NEVER, Ordering::Relaxed);
                return;
            }
            _ => {}
        }
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        COLOR_MODE.store(COLOR_NEVER, Ordering::Relaxed);
        return;
    }
    if std::env::var("CLICOLOR_FORCE").is_ok_and(|v| !v.is_empty() && v != "0") {
        COLOR_MODE.store(COLOR_ALWAYS, Ordering::Relaxed);
        return;
    }
    COLOR_MODE.store(COLOR_AUTO, Ordering::Relaxed);
}

/// Returns `true` when rich ANSI styling should be emitted to stdout.
pub fn colors_enabled() -> bool {
    match COLOR_MODE.load(Ordering::Relaxed) {
        COLOR_ALWAYS => true,
        COLOR_NEVER => false,
        _ => std::io::stdout().is_terminal(),
    }
}

/// Returns `true` when stderr supports ANSI styling.
pub fn stderr_colors_enabled() -> bool {
    match COLOR_MODE.load(Ordering::Relaxed) {
        COLOR_ALWAYS => true,
        COLOR_NEVER => false,
        _ => std::io::stderr().is_terminal(),
    }
}

/// Renders the HD 3D Braille Anvil with a forged-steel vertical ANSI gradient and title bar.
pub fn render_banner(subtitle: Option<&str>) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let sub = subtitle.unwrap_or("Native AI Engineering Harness");
    if !colors_enabled() {
        let mut lines: Vec<String> = ANVIL_ART_LINES.iter().map(|s| (*s).to_owned()).collect();
        lines.push(String::new());
        lines.push(format!("  A N V I L  v{version}  -  {sub}"));
        return lines.join("\n");
    }

    let rst = "\x1b[0m";
    let mut lines = Vec::with_capacity(ANVIL_ART_LINES.len() + 4);
    for (idx, line) in ANVIL_ART_LINES.iter().enumerate() {
        let shade = match idx {
            0..=2 => "\x1b[1;38;5;255m",
            3..=5 => "\x1b[38;5;252m",
            6..=8 => "\x1b[38;5;249m",
            9..=12 => "\x1b[38;5;246m",
            13..=15 => "\x1b[38;5;243m",
            _ => "\x1b[38;5;240m",
        };
        lines.push(format!("{shade}{line}{rst}"));
    }
    lines.push(String::new());
    lines.push(format!(
        "  \x1b[1;38;5;208m◆{rst}  \x1b[1;38;5;214mA N V I L{rst}  \x1b[90mv{version}{rst}  \x1b[90m·{rst}  \x1b[1;97m{sub}{rst}"
    ));
    lines.push(format!(
        "  \x1b[90m──────────────────────────────────────────────────────────────────────────{rst}"
    ));
    lines.join("\n")
}

/// Prints the HD 3D Braille Anvil banner when running in an interactive/colored terminal.
pub fn print_banner_if_tty(subtitle: &str) {
    if colors_enabled() {
        println!("{}\n", render_banner(Some(subtitle)));
    }
}

/// Prints a structured section divider on interactive terminals.
pub fn print_section(title: &str) {
    if colors_enabled() {
        println!("\n\x1b[1;38;5;214m▸\x1b[0m \x1b[1;97m{title}\x1b[0m \x1b[90m──────────────────────────────────────────\x1b[0m");
    }
}

/// Prints an aligned key-value row (`label` padded to 13 chars).
pub fn print_kv(label: &str, value: &str) {
    if colors_enabled() {
        println!("    \x1b[90m{label:<13}\x1b[0m {value}");
    } else {
        println!("  {label:<13} {value}");
    }
}

/// Prints a consistent aligned diagnostic/health row for `doctor` and verification reports.
pub fn print_status_row(badge: &str, label: &str, detail: &str) {
    let padded = format!("{label}:");
    if colors_enabled() {
        println!("  {badge} \x1b[1;97m{padded:<10}\x1b[0m {detail}");
    } else {
        println!("{badge} {label}: {detail}");
    }
}

pub fn bold(text: &str) -> String {
    paint(text, "1;97")
}

pub fn dim(text: &str) -> String {
    paint(text, "90")
}

pub fn accent(text: &str) -> String {
    paint(text, "1;38;5;214")
}

pub fn cyan(text: &str) -> String {
    paint(text, "36")
}

pub fn green(text: &str) -> String {
    paint(text, "1;32")
}

pub fn yellow(text: &str) -> String {
    paint(text, "1;33")
}

pub fn red(text: &str) -> String {
    paint(text, "1;31")
}

pub fn ok_badge() -> String {
    if colors_enabled() {
        "\x1b[1;32m[OK]  \x1b[0m".to_owned()
    } else {
        "[OK]  ".to_owned()
    }
}

pub fn warn_badge() -> String {
    if colors_enabled() {
        "\x1b[1;33m[WARN]\x1b[0m".to_owned()
    } else {
        "[WARN]".to_owned()
    }
}

pub fn fail_badge() -> String {
    if colors_enabled() {
        "\x1b[1;31m[FAIL]\x1b[0m".to_owned()
    } else {
        "[FAIL]".to_owned()
    }
}

pub fn error_badge() -> String {
    if colors_enabled() {
        "\x1b[1;31m[ERROR]\x1b[0m".to_owned()
    } else {
        "[ERROR]".to_owned()
    }
}

pub fn info_badge() -> String {
    if colors_enabled() {
        "\x1b[1;36m[INFO]\x1b[0m".to_owned()
    } else {
        "[INFO]".to_owned()
    }
}

/// Renders a visual progress bar for rule-hit telemetry on TTYs.
pub fn bar(value: u64, max: u64, width: usize) -> String {
    if !colors_enabled() || max == 0 || width == 0 {
        return String::new();
    }
    let filled = ((value as f64 / max as f64) * width as f64).round() as usize;
    let filled = filled.clamp(1, width);
    let empty = width.saturating_sub(filled);
    format!(
        " \x1b[38;5;214m{}\x1b[90m{}\x1b[0m",
        "█".repeat(filled),
        "░".repeat(empty)
    )
}

/// Formats a stderr error prefix with red color when stderr is a TTY.
pub fn stderr_error(msg: &str) -> String {
    if stderr_colors_enabled() {
        format!("\x1b[1;31merror:\x1b[0m {msg}")
    } else {
        format!("error: {msg}")
    }
}

fn paint(text: &str, code: &str) -> String {
    if colors_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

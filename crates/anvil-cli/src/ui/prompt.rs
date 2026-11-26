//! Interactive Clack-style arrow-key TUI prompts for `anvil init`.

use std::io::{self, IsTerminal, Write};

use anyhow::Result;
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, ClearType};

const C_RESET: &str = "\x1b[0m";
const C_DIAMOND: &str = "\x1b[1;38;5;208m";
const C_ACTIVE: &str = "\x1b[1;38;5;214m";
const C_WHITE: &str = "\x1b[1;97m";
const C_CYAN: &str = "\x1b[36m";
const C_GREEN: &str = "\x1b[1;32m";
const C_RED: &str = "\x1b[1;31m";
const C_DIM: &str = "\x1b[90m";
const C_SILVER: &str = "\x1b[38;5;248m";
const C_HINT_ACT: &str = "\x1b[38;5;250m";

/// A selectable option displayed in [`select`] or [`multi_select`].
#[derive(Debug, Clone)]
pub struct SelectOption {
    pub label: String,
    pub hint: String,
}

impl SelectOption {
    /// Creates an option with a primary label and secondary hint.
    pub fn new(label: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { label: label.into(), hint: hint.into() }
    }
}

/// Result of an interactive TUI prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptOutcome<T> {
    Confirmed(T),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    Continue,
    Confirm,
    Cancel,
}

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(err) = execute!(io::stdout(), cursor::Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(err.into());
        }
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Debug, Clone)]
struct SelectState {
    cursor: usize,
    len: usize,
}

impl SelectState {
    fn new(len: usize, default_idx: usize) -> Self {
        let cursor = if len == 0 { 0 } else { default_idx.min(len - 1) };
        Self { cursor, len }
    }

    fn move_up(&mut self) {
        if self.len > 0 {
            self.cursor = if self.cursor == 0 { self.len - 1 } else { self.cursor - 1 };
        }
    }

    fn move_down(&mut self) {
        if self.len > 0 {
            self.cursor = (self.cursor + 1) % self.len;
        }
    }

    fn apply_key(&mut self, code: KeyCode, mods: KeyModifiers) -> KeyAction {
        if is_cancel(code, mods) {
            return KeyAction::Cancel;
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Enter => return KeyAction::Confirm,
            _ => {}
        }
        KeyAction::Continue
    }
}

#[derive(Debug, Clone)]
struct MultiSelectState {
    cursor: usize,
    selected: Vec<bool>,
}

impl MultiSelectState {
    fn new(len: usize, defaults: &[bool]) -> Self {
        let selected = (0..len).map(|i| defaults.get(i).copied().unwrap_or(false)).collect();
        Self { cursor: 0, selected }
    }

    fn move_up(&mut self) {
        let len = self.selected.len();
        if len > 0 {
            self.cursor = if self.cursor == 0 { len - 1 } else { self.cursor - 1 };
        }
    }

    fn move_down(&mut self) {
        let len = self.selected.len();
        if len > 0 {
            self.cursor = (self.cursor + 1) % len;
        }
    }

    fn toggle_current(&mut self) {
        if let Some(slot) = self.selected.get_mut(self.cursor) {
            *slot = !*slot;
        }
    }

    fn toggle_all(&mut self) {
        let target = !self.selected.iter().all(|v| *v);
        self.selected.fill(target);
    }

    fn apply_key(&mut self, code: KeyCode, mods: KeyModifiers) -> KeyAction {
        if is_cancel(code, mods) {
            return KeyAction::Cancel;
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Char(' ') => self.toggle_current(),
            KeyCode::Char('a') => self.toggle_all(),
            KeyCode::Enter => return KeyAction::Confirm,
            _ => {}
        }
        KeyAction::Continue
    }
}

#[derive(Debug, Clone)]
struct TextInputState {
    value: String,
    default_value: String,
}

impl TextInputState {
    fn new(default_value: &str) -> Self {
        Self { value: String::new(), default_value: default_value.to_owned() }
    }

    fn apply_key(&mut self, code: KeyCode, mods: KeyModifiers) -> KeyAction {
        if is_cancel(code, mods) {
            return KeyAction::Cancel;
        }
        match code {
            KeyCode::Enter => return KeyAction::Confirm,
            KeyCode::Backspace => {
                self.value.pop();
            }
            KeyCode::Char(ch)
                if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
            {
                self.value.push(ch);
            }
            _ => {}
        }
        KeyAction::Continue
    }

    fn resolved(&self) -> String {
        let trimmed = self.value.trim();
        if trimmed.is_empty() { self.default_value.clone() } else { trimmed.to_owned() }
    }
}

fn is_cancel(code: KeyCode, mods: KeyModifiers) -> bool {
    matches!(code, KeyCode::Esc)
        || (matches!(code, KeyCode::Char('c' | 'C')) && mods.contains(KeyModifiers::CONTROL))
}

/// Returns `true` when real TTY arrow-key prompts should be rendered.
pub fn is_interactive_tui() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("ANVIL_INIT_FORCE_PROMPT").is_none()
}

/// Interactive single-choice arrow-key menu matching Anvil's forged-steel/amber aesthetic.
pub fn select(
    title: &str,
    collapsed_label: &str,
    subtitle: Option<&str>,
    options: &[SelectOption],
    default_idx: usize,
) -> Result<PromptOutcome<usize>> {
    select_with_mode(title, collapsed_label, subtitle, options, default_idx, true)
}

/// Same arrow-key menu without printing a collapsed summary line (useful for directory navigation).
pub fn select_transient(
    title: &str,
    subtitle: Option<&str>,
    options: &[SelectOption],
    default_idx: usize,
) -> Result<PromptOutcome<usize>> {
    select_with_mode(title, "", subtitle, options, default_idx, false)
}

/// Prints a single completed step on the Clack rail (`◇  <label>` / `│  <value>`).
pub fn emit_collapsed(label: &str, summary: &str) -> Result<()> {
    print_collapsed(label, summary)
}

fn select_with_mode(
    title: &str,
    collapsed_label: &str,
    subtitle: Option<&str>,
    options: &[SelectOption],
    default_idx: usize,
    collapse: bool,
) -> Result<PromptOutcome<usize>> {
    if options.is_empty() {
        return Ok(PromptOutcome::Cancelled);
    }
    let mut state = SelectState::new(options.len(), default_idx);
    let outcome = run_loop(|out, prev| {
        let lines = render_select_frame(title, subtitle, options, state.cursor);
        draw_frame(out, *prev, &lines)?;
        *prev = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        read_key_action(|c, m| state.apply_key(c, m))
    })?;
    if outcome == KeyAction::Cancel {
        print_cancelled()?;
        return Ok(PromptOutcome::Cancelled);
    }
    if collapse {
        print_collapsed(collapsed_label, &options[state.cursor].label)?;
    }
    Ok(PromptOutcome::Confirmed(state.cursor))
}

/// Interactive multi-choice checklist (`Space` toggle, `a` toggle-all, `Enter` confirm).
pub fn multi_select(
    title: &str,
    collapsed_label: &str,
    subtitle: Option<&str>,
    options: &[SelectOption],
    defaults: &[bool],
) -> Result<PromptOutcome<Vec<bool>>> {
    if options.is_empty() {
        return Ok(PromptOutcome::Confirmed(Vec::new()));
    }
    let mut state = MultiSelectState::new(options.len(), defaults);
    let outcome = run_loop(|out, prev| {
        let lines = render_multi_frame(title, subtitle, options, &state);
        draw_frame(out, *prev, &lines)?;
        *prev = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        read_key_action(|c, m| state.apply_key(c, m))
    })?;
    if outcome == KeyAction::Cancel {
        print_cancelled()?;
        return Ok(PromptOutcome::Cancelled);
    }
    let summary = format_multi_summary(options, &state.selected);
    print_collapsed(collapsed_label, &summary)?;
    Ok(PromptOutcome::Confirmed(state.selected))
}

/// Interactive single-line text input styled on the same Clack rail.
pub fn text_input(
    title: &str,
    collapsed_label: &str,
    default_value: &str,
) -> Result<PromptOutcome<String>> {
    let mut state = TextInputState::new(default_value);
    let outcome = run_loop(|out, prev| {
        let lines = render_text_frame(title, &state);
        draw_frame(out, *prev, &lines)?;
        *prev = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        read_key_action(|c, m| state.apply_key(c, m))
    })?;
    if outcome == KeyAction::Cancel {
        print_cancelled()?;
        return Ok(PromptOutcome::Cancelled);
    }
    let val = state.resolved();
    print_collapsed(collapsed_label, &val)?;
    Ok(PromptOutcome::Confirmed(val))
}

fn run_loop<F>(mut step: F) -> Result<KeyAction>
where
    F: FnMut(&mut io::Stdout, &mut u16) -> Result<KeyAction>,
{
    let _guard = RawModeGuard::enter()?;
    let mut out = io::stdout();
    let mut prev_lines: u16 = 0;
    loop {
        let action = step(&mut out, &mut prev_lines)?;
        if action != KeyAction::Continue {
            clear_frame(&mut out, prev_lines)?;
            return Ok(action);
        }
    }
}

fn read_key_action<F>(mut on_key: F) -> Result<KeyAction>
where
    F: FnMut(KeyCode, KeyModifiers) -> KeyAction,
{
    loop {
        if let Event::Key(ev) = event::read()? {
            if ev.kind == KeyEventKind::Release {
                continue;
            }
            return Ok(on_key(ev.code, ev.modifiers));
        }
    }
}

fn draw_frame(out: &mut io::Stdout, prev_lines: u16, lines: &[String]) -> Result<()> {
    clear_frame(out, prev_lines)?;
    for line in lines {
        write!(out, "{line}\r\n")?;
    }
    out.flush()?;
    Ok(())
}

fn clear_frame(out: &mut io::Stdout, prev_lines: u16) -> Result<()> {
    execute!(out, cursor::MoveToColumn(0))?;
    if prev_lines > 0 {
        execute!(out, cursor::MoveUp(prev_lines))?;
    }
    execute!(out, terminal::Clear(ClearType::FromCursorDown))?;
    out.flush()?;
    Ok(())
}

fn render_select_frame(
    title: &str,
    subtitle: Option<&str>,
    options: &[SelectOption],
    cursor_idx: usize,
) -> Vec<String> {
    const MAX_ROWS: usize = 14;
    let cols = term_cols();
    let label_width = max_label_width(options, cols);
    let mut lines = render_header(title, subtitle, cols);
    let (start, end) = visible_window(options.len(), cursor_idx, MAX_ROWS);
    if start > 0 {
        lines.push(format!("  {C_DIM}│    ↑ {start} more above{C_RESET}"));
    }
    for (idx, opt) in options.iter().enumerate().take(end).skip(start) {
        let active = idx == cursor_idx;
        let pointer = if active { format!("{C_DIAMOND}▸{C_RESET}") } else { " ".to_owned() };
        let bullet =
            if active { format!("{C_ACTIVE}●{C_RESET}") } else { format!("{C_DIM}○{C_RESET}") };
        lines.push(format_option_row(opt, active, &pointer, &bullet, label_width, cols));
    }
    if end < options.len() {
        lines.push(format!("  {C_DIM}│    ↓ {} more below{C_RESET}", options.len() - end));
    }
    lines.push(format!("  {C_DIM}│{C_RESET}"));
    lines.push(format_footer("↑/↓ move · Enter confirm · Esc cancel", cols));
    lines
}

fn visible_window(len: usize, cursor: usize, max_rows: usize) -> (usize, usize) {
    if len <= max_rows {
        return (0, len);
    }
    let start = cursor.saturating_sub(max_rows / 2).min(len - max_rows);
    (start, start + max_rows)
}

fn render_multi_frame(
    title: &str,
    subtitle: Option<&str>,
    options: &[SelectOption],
    state: &MultiSelectState,
) -> Vec<String> {
    let cols = term_cols();
    let label_width = max_label_width(options, cols);
    let mut lines = render_header(title, subtitle, cols);
    for (idx, opt) in options.iter().enumerate() {
        let active = idx == state.cursor;
        let checked = state.selected.get(idx).copied().unwrap_or(false);
        let pointer = if active { format!("{C_DIAMOND}▸{C_RESET}") } else { " ".to_owned() };
        let mark =
            if checked { format!("{C_GREEN}■{C_RESET}") } else { format!("{C_DIM}□{C_RESET}") };
        lines.push(format_option_row(opt, active, &pointer, &mark, label_width, cols));
    }
    lines.push(format!("  {C_DIM}│{C_RESET}"));
    lines.push(format_footer(
        "↑/↓ move · Space toggle · a toggle all · Enter confirm · Esc cancel",
        cols,
    ));
    lines
}

fn render_text_frame(title: &str, state: &TextInputState) -> Vec<String> {
    let cols = term_cols();
    let mut lines = render_header(title, None, cols);
    let max_body = cols.saturating_sub(10);
    let shown = if state.value.is_empty() {
        let ph = truncate_plain(&state.default_value, max_body);
        format!("{C_DIM}{ph}{C_RESET}{C_ACTIVE}█{C_RESET}")
    } else {
        let val = truncate_plain(&state.value, max_body);
        format!("{C_CYAN}{val}{C_RESET}{C_ACTIVE}█{C_RESET}")
    };
    lines.push(format!("  {C_DIM}│{C_RESET}  {C_DIAMOND}▸{C_RESET} {shown}"));
    lines.push(format!("  {C_DIM}│{C_RESET}"));
    lines.push(format_footer("Type URL or press Enter for default · Esc cancel", cols));
    lines
}

fn render_header(title: &str, subtitle: Option<&str>, cols: usize) -> Vec<String> {
    let mut lines = Vec::with_capacity(3);
    let t = truncate_plain(title, cols.saturating_sub(6));
    lines.push(format!("  {C_DIAMOND}◆{C_RESET}  {C_WHITE}{t}{C_RESET}"));
    if let Some(sub) = subtitle {
        let s = truncate_plain(sub, cols.saturating_sub(6));
        lines.push(format!("  {C_DIM}│{C_RESET}  {C_DIM}{s}{C_RESET}"));
    }
    lines.push(format!("  {C_DIM}│{C_RESET}"));
    lines
}

fn format_option_row(
    opt: &SelectOption,
    active: bool,
    pointer: &str,
    glyph: &str,
    label_width: usize,
    cols: usize,
) -> String {
    let label_plain = truncate_plain(&opt.label, label_width);
    let pad = " ".repeat(label_width.saturating_sub(label_plain.chars().count()));
    let avail_hint = cols.saturating_sub(11 + label_width);
    let hint_plain = if opt.hint.is_empty() || avail_hint < 6 {
        String::new()
    } else {
        format!("  {}", truncate_plain(&opt.hint, avail_hint.saturating_sub(2)))
    };
    let (lc, hc) = if active { (C_ACTIVE, C_HINT_ACT) } else { (C_SILVER, C_DIM) };
    format!(
        "  {C_DIM}│{C_RESET}  {pointer} {glyph} {lc}{label_plain}{C_RESET}{pad}{hc}{hint_plain}{C_RESET}"
    )
}

fn format_footer(hint: &str, cols: usize) -> String {
    let h = truncate_plain(hint, cols.saturating_sub(6));
    format!("  {C_DIM}└  {h}{C_RESET}")
}

fn print_collapsed(label: &str, summary: &str) -> Result<()> {
    let mut out = io::stdout();
    writeln!(out, "  {C_GREEN}◇{C_RESET}  {C_WHITE}{label}{C_RESET}")?;
    writeln!(out, "  {C_DIM}│{C_RESET}  {C_CYAN}{summary}{C_RESET}")?;
    writeln!(out, "  {C_DIM}│{C_RESET}")?;
    out.flush()?;
    Ok(())
}

fn print_cancelled() -> Result<()> {
    let mut out = io::stdout();
    writeln!(out, "  {C_RED}■{C_RESET}  {C_DIM}Initialization cancelled{C_RESET}")?;
    out.flush()?;
    Ok(())
}

fn format_multi_summary(options: &[SelectOption], selected: &[bool]) -> String {
    let chosen: Vec<&str> = options
        .iter()
        .zip(selected.iter())
        .filter_map(|(o, on)| on.then_some(o.label.as_str()))
        .collect();
    if chosen.is_empty() { "None (SonarQube disabled)".to_owned() } else { chosen.join(", ") }
}

fn max_label_width(options: &[SelectOption], cols: usize) -> usize {
    let raw_max = options.iter().map(|o| o.label.chars().count()).max().unwrap_or(12);
    raw_max.min(cols.saturating_sub(18).max(12))
}

fn term_cols() -> usize {
    terminal::size().map(|(w, _)| usize::from(w)).unwrap_or(80).max(40)
}

fn truncate_plain(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_cols {
        return s.to_owned();
    }
    if max_cols == 1 {
        return "…".to_owned();
    }
    let mut out: String = s.chars().take(max_cols - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_state_wraps_and_confirms() {
        let mut st = SelectState::new(3, 0);
        assert_eq!(st.apply_key(KeyCode::Up, KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.cursor, 2);
        assert_eq!(st.apply_key(KeyCode::Down, KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.cursor, 0);
        assert_eq!(st.apply_key(KeyCode::Char('j'), KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.cursor, 1);
        assert_eq!(st.apply_key(KeyCode::Char('k'), KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.cursor, 0);
        assert_eq!(st.apply_key(KeyCode::Enter, KeyModifiers::NONE), KeyAction::Confirm);
        assert_eq!(st.apply_key(KeyCode::Esc, KeyModifiers::NONE), KeyAction::Cancel);
        assert_eq!(st.apply_key(KeyCode::Char('c'), KeyModifiers::CONTROL), KeyAction::Cancel);
    }

    #[test]
    fn multi_select_toggles_item_and_all() {
        let mut st = MultiSelectState::new(3, &[false, false, false]);
        assert_eq!(st.apply_key(KeyCode::Char(' '), KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.selected, vec![true, false, false]);
        assert_eq!(st.apply_key(KeyCode::Down, KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.cursor, 1);
        assert_eq!(st.apply_key(KeyCode::Char('a'), KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.selected, vec![true, true, true]);
        assert_eq!(st.apply_key(KeyCode::Char('a'), KeyModifiers::NONE), KeyAction::Continue);
        assert_eq!(st.selected, vec![false, false, false]);
    }

    #[test]
    fn text_input_resolves_default_or_typed() {
        let mut st = TextInputState::new("http://localhost:9000");
        assert_eq!(st.resolved(), "http://localhost:9000");
        st.apply_key(KeyCode::Char('h'), KeyModifiers::NONE);
        st.apply_key(KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(st.resolved(), "hi");
        st.apply_key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(st.resolved(), "h");
        assert_eq!(st.apply_key(KeyCode::Enter, KeyModifiers::NONE), KeyAction::Confirm);
    }

    #[test]
    fn truncate_plain_handles_unicode_safely() {
        assert_eq!(truncate_plain("áéíóú", 4), "áéí…");
        assert_eq!(truncate_plain("abc", 5), "abc");
        assert_eq!(truncate_plain("abc", 1), "…");
        assert_eq!(truncate_plain("abc", 0), "");
    }
}

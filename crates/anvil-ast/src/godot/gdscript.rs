//! Pure-Rust GDScript (`.gd`) symbol/edge extractor and Tier 0 Godot 4 rules.

use std::sync::OnceLock;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{
    Diagnostic, EdgeKind, EdgeRef, FileAstFacts, Language, Severity, SymbolDef, SymbolKind,
};
use regex::Regex;

use super::{has_godot_allow, is_ident_char, push_diag};
use crate::extractors::ExtractState;

static PRELOAD_RE: OnceLock<Option<Regex>> = OnceLock::new();
static CONNECT_RE: OnceLock<Option<Regex>> = OnceLock::new();
static GD3_IDENT_RE: OnceLock<Option<Regex>> = OnceLock::new();
static GD3_CALL_RE: OnceLock<Option<Regex>> = OnceLock::new();
static GD3_KW_RE: OnceLock<Option<Regex>> = OnceLock::new();
static TODO_RE: OnceLock<Option<Regex>> = OnceLock::new();

struct GdLine<'a> {
    line: u32,
    raw: &'a str,
    raw_code: &'a str,
    code: String,
    comment: Option<&'a str>,
    starts_in_str: bool,
}

struct GdContext<'a> {
    file: &'a str,
    lines: &'a [GdLine<'a>],
    comments: Vec<Option<&'a str>>,
    config: &'a AnvilConfig,
    extends_character_body: bool,
    symbols: Vec<SymbolDef>,
    edges: Vec<EdgeRef>,
    diagnostics: Vec<Diagnostic>,
}

/// Analyzes a GDScript (`.gd`) source file.
pub fn analyze(
    normalized_file: &str,
    source: &str,
    blake3_hash: String,
    config: &AnvilConfig,
) -> FileAstFacts {
    let lines = scan_gd_lines(source);
    let comments = lines.iter().map(|l| l.comment).collect();
    let extends_character_body = detects_character_body(&lines);
    let mut ctx = GdContext {
        file: normalized_file,
        lines: &lines,
        comments,
        config,
        extends_character_body,
        symbols: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
    };

    extract_all(&mut ctx);
    run_all_rules(&mut ctx);

    FileAstFacts {
        path: normalized_file.to_owned(),
        blake3_hash,
        language: Language::GdScript,
        symbols: ctx.symbols,
        edges: ctx.edges,
        diagnostics: ctx.diagnostics,
    }
}

fn scan_gd_lines(source: &str) -> Vec<GdLine<'_>> {
    let mut result = Vec::new();
    let mut in_triple: Option<u8> = None;

    for (idx, raw_line) in source.split('\n').enumerate() {
        let raw = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let starts_in_str = in_triple.is_some();
        let (raw_code, code, comment) = scan_single_line(raw, &mut in_triple);
        result.push(GdLine { line: idx as u32 + 1, raw, raw_code, code, comment, starts_in_str });
    }
    result
}

fn scan_single_line<'a>(
    raw: &'a str,
    in_triple: &mut Option<u8>,
) -> (&'a str, String, Option<&'a str>) {
    let bytes = raw.as_bytes();
    let mut code = String::with_capacity(raw.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if let Some(q) = *in_triple {
            i = advance_triple_str(bytes, i, q, in_triple, &mut code);
            continue;
        }
        if bytes[i] == b'#' {
            return (&raw[..i], code, Some(&raw[i..]));
        }
        if is_triple_quote(bytes, i) {
            let q = bytes[i];
            code.push_str(&raw[i..i + 3]);
            *in_triple = Some(q);
            i += 3;
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            i = advance_single_str(raw, bytes, i, &mut code);
            continue;
        }
        let ch = raw[i..].chars().next().unwrap_or(' ');
        code.push(ch);
        i += ch.len_utf8();
    }
    (raw, code, None)
}

fn is_triple_quote(bytes: &[u8], i: usize) -> bool {
    i + 2 < bytes.len()
        && (bytes[i] == b'"' || bytes[i] == b'\'')
        && bytes[i + 1] == bytes[i]
        && bytes[i + 2] == bytes[i]
}

fn advance_triple_str(
    bytes: &[u8],
    mut i: usize,
    q: u8,
    in_triple: &mut Option<u8>,
    code: &mut String,
) -> usize {
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            code.push_str("  ");
            i += 2;
            continue;
        }
        if i + 2 < bytes.len() && bytes[i] == q && bytes[i + 1] == q && bytes[i + 2] == q {
            code.push_str(if q == b'"' { "\"\"\"" } else { "'''" });
            *in_triple = None;
            return i + 3;
        }
        code.push(' ');
        i += 1;
    }
    i
}

fn advance_single_str(raw: &str, bytes: &[u8], mut i: usize, code: &mut String) -> usize {
    let q = bytes[i];
    code.push(q as char);
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            code.push_str("  ");
            i += 2;
            continue;
        }
        if bytes[i] == q {
            code.push(q as char);
            return i + 1;
        }
        let ch = raw[i..].chars().next().unwrap_or(' ');
        for _ in 0..ch.len_utf8() {
            code.push(' ');
        }
        i += ch.len_utf8();
    }
    i
}

fn detects_character_body(lines: &[GdLine<'_>]) -> bool {
    lines.iter().any(|l| {
        if l.starts_in_str {
            return false;
        }
        let trimmed = l.code.trim();
        matches!(
            trimmed.strip_prefix("extends").map(str::trim),
            Some("CharacterBody2D" | "CharacterBody3D")
        )
    })
}

fn extract_all(ctx: &mut GdContext<'_>) {
    let mut inner_class: Option<(String, usize)> = None;
    let mut active_func: Option<(String, u32)> = None;
    let mut prev_has_export = false;

    for (idx, line) in ctx.lines.iter().enumerate() {
        if line.starts_in_str {
            continue;
        }
        let indent = leading_indent_level(line.raw);
        if let Some((_, cls_indent)) = &inner_class
            && !line.code.trim().is_empty()
            && indent <= *cls_indent
        {
            inner_class = None;
        }
        if let Some((_, func_end)) = &active_func
            && line.line > *func_end
        {
            active_func = None;
        }

        let trimmed = line.code.trim();
        if trimmed.is_empty() {
            continue;
        }

        extract_line_decls(
            ctx,
            idx,
            trimmed,
            indent,
            &mut inner_class,
            &mut active_func,
            &mut prev_has_export,
        );
        let source_sym = active_func.as_ref().map(|(n, _)| n.clone());
        extract_preloads(ctx, line, source_sym.as_deref());
        extract_connects(ctx, line, source_sym.as_deref());
    }
}

/// Records a function, or a method when it sits inside an inner class; returns its name and end.
fn push_gd_func(
    ctx: &mut GdContext<'_>,
    idx: usize,
    indent: usize,
    func_name: String,
    inner_class: Option<&(String, usize)>,
) -> (String, u32) {
    let line_no = ctx.lines[idx].line;
    let end_line = compute_block_end_line(ctx.lines, idx, indent);
    let (kind, qualified) = match inner_class {
        Some((cls, _)) if indent > 0 => (SymbolKind::Method, format!("{cls}.{func_name}")),
        _ => (SymbolKind::Function, func_name.clone()),
    };
    let exported = !func_name.starts_with('_');
    push_gd_symbol(ctx, func_name, qualified.clone(), kind, (line_no, end_line), exported);
    (qualified, end_line)
}

fn extract_line_decls(
    ctx: &mut GdContext<'_>,
    idx: usize,
    trimmed: &str,
    indent: usize,
    inner_class: &mut Option<(String, usize)>,
    active_func: &mut Option<(String, u32)>,
    prev_has_export: &mut bool,
) {
    let line = &ctx.lines[idx];
    if let Some(target) = extract_extends_target(line.raw_code) {
        ctx.edges.push(EdgeRef {
            source_file: ctx.file.to_owned(),
            source_symbol: None,
            target_path_or_symbol: target,
            kind: EdgeKind::Extends,
            line: line.line,
        });
    } else if let Some(name) = extract_after_kw(trimmed, "class_name") {
        push_gd_symbol(ctx, name.clone(), name, SymbolKind::Class, (line.line, line.line), true);
    } else if let Some(name) = extract_after_kw(trimmed, "class") {
        let end_line = compute_block_end_line(ctx.lines, idx, indent);
        push_gd_symbol(
            ctx,
            name.clone(),
            name.clone(),
            SymbolKind::Class,
            (line.line, end_line),
            !name.starts_with('_'),
        );
        *inner_class = Some((name, indent));
    } else if let Some(name) = extract_after_kw(trimmed, "signal") {
        push_gd_symbol(ctx, name.clone(), name, SymbolKind::Variable, (line.line, line.line), true);
    } else if let Some(func_name) = extract_func_name(trimmed) {
        *active_func = Some(push_gd_func(ctx, idx, indent, func_name, inner_class.as_ref()));
    } else if let Some(var_name) = extract_export_var_name(trimmed, *prev_has_export) {
        push_gd_symbol(
            ctx,
            var_name.clone(),
            var_name,
            SymbolKind::Variable,
            (line.line, line.line),
            true,
        );
    }

    *prev_has_export = trimmed.starts_with("@export");
}

fn push_gd_symbol(
    ctx: &mut GdContext<'_>,
    name: String,
    qualified_name: String,
    kind: SymbolKind,
    span: (u32, u32),
    exported: bool,
) {
    if name.is_empty() {
        return;
    }
    ctx.symbols.push(SymbolDef {
        name,
        qualified_name,
        kind,
        file: ctx.file.to_owned(),
        line: span.0,
        end_line: span.1.max(span.0),
        signature: None,
        exported,
    });
}

fn extract_extends_target(raw_code: &str) -> Option<String> {
    let trimmed = raw_code.trim();
    let rest = trimmed.strip_prefix("extends")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let raw_target = rest.trim();
    let unquoted = ExtractState::unquote(raw_target);
    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

fn extract_after_kw(trimmed: &str, kw: &str) -> Option<String> {
    let rest = trimmed.strip_prefix(kw)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    read_ident(rest.trim_start())
}

fn extract_func_name(trimmed: &str) -> Option<String> {
    let stmt = strip_gd_prefixes(trimmed);
    let rest = stmt.strip_prefix("func")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let after = rest.trim_start();
    let name = read_ident(after)?;
    after[name.len()..].trim_start().starts_with('(').then_some(name)
}

fn extract_export_var_name(trimmed: &str, prev_has_export: bool) -> Option<String> {
    let has_export = trimmed.starts_with("@export") || prev_has_export;
    if !has_export {
        return None;
    }
    let stmt = strip_gd_prefixes(trimmed);
    let rest = stmt.strip_prefix("var")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    read_ident(rest.trim_start())
}

fn read_ident(s: &str) -> Option<String> {
    let len = s.bytes().take_while(|&b| is_ident_char(b)).count();
    (len > 0).then(|| s[..len].to_owned())
}

fn strip_gd_prefixes(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(after_at) = s.strip_prefix('@') {
            let ident_len = after_at.bytes().take_while(|&b| is_ident_char(b)).count();
            if ident_len == 0 {
                return s;
            }
            let mut rest = &after_at[ident_len..];
            if rest.starts_with('(')
                && let Some(close_idx) = find_matching_paren(rest)
            {
                rest = &rest[close_idx + 1..];
            }
            s = rest;
            continue;
        }
        if let Some(after_static) = s.strip_prefix("static")
            && after_static.starts_with(char::is_whitespace)
        {
            s = after_static;
            continue;
        }
        return s;
    }
}

fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, b) in s.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn leading_indent_level(raw: &str) -> usize {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    for b in raw.bytes() {
        match b {
            b'\t' => tabs += 1,
            b' ' => spaces += 1,
            _ => break,
        }
    }
    tabs * 4 + spaces
}

fn compute_block_end_line(lines: &[GdLine<'_>], start_idx: usize, header_indent: usize) -> u32 {
    let start_line = lines[start_idx].line;
    let mut paren_depth = paren_delta(&lines[start_idx].code);
    let mut last_body_line = start_line;

    for line in &lines[start_idx + 1..] {
        if line.starts_in_str {
            last_body_line = line.line;
            continue;
        }
        let trimmed = line.code.trim();
        if paren_depth > 0 {
            paren_depth = (paren_depth + paren_delta(&line.code)).max(0);
            if !trimmed.is_empty() {
                last_body_line = line.line;
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if leading_indent_level(line.raw) <= header_indent {
            break;
        }
        last_body_line = line.line;
    }
    last_body_line
}

fn paren_delta(code: &str) -> isize {
    code.bytes().fold(0isize, |acc, b| match b {
        b'(' => acc + 1,
        b')' => acc - 1,
        _ => acc,
    })
}

fn extract_preloads(ctx: &mut GdContext<'_>, line: &GdLine<'_>, source_sym: Option<&str>) {
    let Some(re) = PRELOAD_RE
        .get_or_init(|| Regex::new(r#"\b(?:preload|load)\s*\(\s*["']([^"']+)["']\s*\)"#).ok())
        .as_ref()
    else {
        return;
    };
    for caps in re.captures_iter(line.raw_code) {
        let Some(full) = caps.get(0) else { continue };
        let Some(target) = caps.get(1) else { continue };
        if is_inside_outer_string(&line.code, full.start()) {
            continue;
        }
        ctx.edges.push(EdgeRef {
            source_file: ctx.file.to_owned(),
            source_symbol: source_sym.map(str::to_owned),
            target_path_or_symbol: target.as_str().to_owned(),
            kind: EdgeKind::Imports,
            line: line.line,
        });
    }
}

fn extract_connects(ctx: &mut GdContext<'_>, line: &GdLine<'_>, source_sym: Option<&str>) {
    let Some(re) = CONNECT_RE.get_or_init(|| Regex::new(r"\.connect\s*\(([^)]+)\)").ok()).as_ref()
    else {
        return;
    };
    for caps in re.captures_iter(line.raw_code) {
        let Some(full) = caps.get(0) else { continue };
        let Some(args) = caps.get(1) else { continue };
        if is_inside_outer_string(&line.code, full.start()) {
            continue;
        }
        if let Some(target) = parse_connect_target(args.as_str())
            && let Some(clean) = ExtractState::sanitize_call_target(target)
        {
            ctx.edges.push(EdgeRef {
                source_file: ctx.file.to_owned(),
                source_symbol: source_sym.map(str::to_owned),
                target_path_or_symbol: clean,
                kind: EdgeKind::Calls,
                line: line.line,
            });
        }
    }
}

fn parse_connect_target(args: &str) -> Option<String> {
    let parts: Vec<&str> = args.split(',').map(str::trim).collect();
    if parts.is_empty() || parts[0].is_empty() {
        return None;
    }
    if parts.len() >= 3 && (parts[0].starts_with('"') || parts[0].starts_with('\'')) {
        return Some(ExtractState::unquote(parts[2]).to_owned());
    }
    let first = parts[0];
    if let Some(inner) = first.strip_prefix("Callable(") {
        let method = parts.get(1).copied().unwrap_or(inner).trim_end_matches(')');
        return Some(ExtractState::unquote(method).to_owned());
    }
    let base = first.split(".bind(").next().unwrap_or(first).trim();
    Some(ExtractState::unquote(base).to_owned())
}

fn is_inside_outer_string(masked_code: &str, byte_idx: usize) -> bool {
    let prefix = masked_code.get(..byte_idx).unwrap_or(masked_code);
    let mut in_dq = false;
    let mut in_sq = false;
    for b in prefix.bytes() {
        match b {
            b'"' if !in_sq => in_dq = !in_dq,
            b'\'' if !in_dq => in_sq = !in_sq,
            _ => {}
        }
    }
    in_dq || in_sq
}

fn run_all_rules(ctx: &mut GdContext<'_>) {
    let mut idx = 0usize;
    while idx < ctx.lines.len() {
        let line = &ctx.lines[idx];
        check_todo_comment(ctx, line);

        if line.starts_in_str {
            idx += 1;
            continue;
        }
        check_tabs_only(ctx, line);
        check_godot3_syntax(ctx, line);
        check_var_and_velocity(ctx, line);

        let stmt = strip_gd_prefixes(line.code.trim_start());
        if starts_with_kw(stmt, "func") && has_named_func_paren(stmt) {
            idx = check_func_typing(ctx, idx);
            continue;
        }
        idx += 1;
    }
}

fn starts_with_kw(s: &str, kw: &str) -> bool {
    s.strip_prefix(kw).is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

fn has_named_func_paren(stmt: &str) -> bool {
    let after = stmt[4..].trim_start();
    let Some(name) = read_ident(after) else {
        return false;
    };
    after[name.len()..].trim_start().starts_with('(')
}

fn check_tabs_only(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    let trimmed = line.raw.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || !line.raw.starts_with(' ') {
        return;
    }
    if has_godot_allow(&ctx.comments, line.line, "godot4-tabs-only", ctx.config) {
        return;
    }
    push_diag(
        &mut ctx.diagnostics,
        "godot4-tabs-only",
        Severity::Warning,
        ctx.file,
        (line.line, 1),
        "GDScript indentation should use tabs instead of leading spaces".to_owned(),
        Some("Replace leading spaces with tab characters"),
    );
}

fn check_todo_comment(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    let Some(comment) = line.comment else { return };
    let Some(re) = TODO_RE.get_or_init(|| Regex::new(r"\b(TODO|FIXME|HACK|XXX)\b").ok()).as_ref()
    else {
        return;
    };
    let Some(mat) = re.find(comment) else { return };
    if has_godot_allow(&ctx.comments, line.line, "no-todo-comments", ctx.config) {
        return;
    }
    let col = (line.raw.len() - comment.len() + mat.start() + 1) as u32;
    push_diag(
        &mut ctx.diagnostics,
        "no-todo-comments",
        Severity::Error,
        ctx.file,
        (line.line, col),
        format!("Disallowed `{}` marker in comment", mat.as_str()),
        Some("Resolve the work item or suppress with `# anvil-allow(no-todo-comments): <reason>`"),
    );
}

fn check_godot3_syntax(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    if has_godot_allow(&ctx.comments, line.line, "godot4-no-godot3-syntax", ctx.config) {
        return;
    }
    check_gd3_keywords(ctx, line);
    check_gd3_calls(ctx, line);
    check_gd3_identifiers(ctx, line);
    check_move_and_slide_args(ctx, line);
}

fn check_gd3_keywords(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    if line.code.trim() == "tool" {
        let col = line.code.find("tool").map_or(1, |i| i as u32 + 1);
        push_gd3_diag(ctx, line.line, col, "tool", "Use `@tool` in Godot 4");
    }
    let Some(re) = GD3_KW_RE
        .get_or_init(|| Regex::new(r"\bonready\s+var\b|\bexport\s+var\b|\bexport\s*\(").ok())
        .as_ref()
    else {
        return;
    };
    for mat in re.find_iter(&line.code) {
        let prev = mat.start().checked_sub(1).and_then(|i| line.code.as_bytes().get(i).copied());
        if matches!(prev, Some(b'@' | b'.')) {
            continue;
        }
        let text = mat.as_str();
        let hint = if text.starts_with("onready") {
            "Use `@onready var` in Godot 4"
        } else {
            "Use `@export` annotations in Godot 4"
        };
        push_gd3_diag(ctx, line.line, mat.start() as u32 + 1, text, hint);
    }
}

fn check_gd3_calls(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    let Some(re) = GD3_CALL_RE
        .get_or_init(|| {
            Regex::new(
                r"(\.instance\s*\(|\byield\s*\(|\bdeg2rad\s*\(|\brad2deg\s*\(|\brand_range\s*\()",
            )
            .ok()
        })
        .as_ref()
    else {
        return;
    };
    for mat in re.find_iter(&line.code) {
        let text = mat.as_str();
        if text.starts_with("yield")
            && mat.start() > 0
            && line.code.as_bytes().get(mat.start() - 1) == Some(&b'.')
        {
            continue;
        }
        let hint = match text.trim_start_matches('.').split('(').next().unwrap_or("") {
            "instance" => "Use `.instantiate()` in Godot 4",
            "yield" => "Use `await` in Godot 4",
            "deg2rad" => "Use `deg_to_rad()` in Godot 4",
            "rad2deg" => "Use `rad_to_deg()` in Godot 4",
            _ => "Use `randf_range()` or `randi_range()` in Godot 4",
        };
        push_gd3_diag(ctx, line.line, mat.start() as u32 + 1, text.trim_end(), hint);
    }
}

fn check_gd3_identifiers(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    let Some(re) = GD3_IDENT_RE
        .get_or_init(|| {
            Regex::new(r"\b(?:Spatial|KinematicBody2D|KinematicBody|MeshInstance|SpatialMaterial|AnimatedSprite|Position2D|Position3D|OS\.window_size)\b").ok()
        })
        .as_ref()
    else {
        return;
    };
    for mat in re.find_iter(&line.code) {
        let matched = mat.as_str();
        let hint = match matched {
            "Spatial" => "Use `Node3D` in Godot 4",
            "KinematicBody2D" => "Use `CharacterBody2D` in Godot 4",
            "KinematicBody" => "Use `CharacterBody3D` in Godot 4",
            "MeshInstance" => "Use `MeshInstance3D` in Godot 4",
            "SpatialMaterial" => "Use `StandardMaterial3D` in Godot 4",
            "AnimatedSprite" => "Use `AnimatedSprite2D` in Godot 4",
            "Position2D" => "Use `Marker2D` in Godot 4",
            "Position3D" => "Use `Marker3D` in Godot 4",
            _ => "Use `DisplayServer.window_get_size()` or `get_window().size` in Godot 4",
        };
        push_gd3_diag(ctx, line.line, mat.start() as u32 + 1, matched, hint);
    }
}

fn check_move_and_slide_args(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    let mut search_from = 0usize;
    while let Some(rel) = line.code[search_from..].find("move_and_slide") {
        let pos = search_from + rel;
        search_from = pos + "move_and_slide".len();
        let prev_ok = pos == 0 || !is_ident_char(line.code.as_bytes()[pos - 1]);
        if !prev_ok {
            continue;
        }
        let after = line.code[search_from..].trim_start();
        if !after.starts_with('(') {
            continue;
        }
        let has_args = match find_matching_paren(after) {
            Some(close_idx) => !after[1..close_idx].trim().is_empty(),
            None => !after[1..].trim().is_empty(),
        };
        if has_args {
            push_gd3_diag(
                ctx,
                line.line,
                pos as u32 + 1,
                "move_and_slide(...)",
                "Godot 4 `move_and_slide()` takes zero arguments; assign `velocity` before calling",
            );
        }
    }
}

fn push_gd3_diag(ctx: &mut GdContext<'_>, line: u32, col: u32, token: &str, hint: &str) {
    push_diag(
        &mut ctx.diagnostics,
        "godot4-no-godot3-syntax",
        Severity::Error,
        ctx.file,
        (line, col),
        format!("Godot 3 syntax `{token}` is not valid in Godot 4"),
        Some(hint),
    );
}

fn check_var_and_velocity(ctx: &mut GdContext<'_>, line: &GdLine<'_>) {
    let stmt = strip_gd_prefixes(line.code.trim_start());
    if !starts_with_kw(stmt, "var") {
        return;
    }
    let after_var = stmt[3..].trim_start();
    let Some(var_name) = read_ident(after_var) else {
        return;
    };
    let col = (line.code.len() - stmt.len() + 1) as u32;

    if ctx.extends_character_body
        && var_name == "velocity"
        && !has_godot_allow(&ctx.comments, line.line, "godot4-characterbody-velocity", ctx.config)
    {
        push_diag(
            &mut ctx.diagnostics,
            "godot4-characterbody-velocity",
            Severity::Error,
            ctx.file,
            (line.line, col),
            "Do not redeclare `var velocity` in a CharacterBody2D/CharacterBody3D script"
                .to_owned(),
            Some("Use the built-in `velocity` property directly"),
        );
    }

    if has_godot_allow(&ctx.comments, line.line, "godot4-static-typing", ctx.config) {
        return;
    }
    check_var_type_annotation(ctx, line.line, col, &var_name, &after_var[var_name.len()..]);
}

fn check_var_type_annotation(
    ctx: &mut GdContext<'_>,
    line: u32,
    col: u32,
    var_name: &str,
    rest_raw: &str,
) {
    let rest = rest_raw.trim_start();
    if let Some(after_inferred) = rest.strip_prefix(":=") {
        let rhs = after_inferred.trim_start();
        let is_null = rhs
            .strip_prefix("null")
            .is_some_and(|tail| tail.is_empty() || !is_ident_char(tail.as_bytes()[0]));
        if is_null {
            push_typing_diag(
                ctx,
                line,
                col,
                format!("Cannot infer static type for `{var_name}` from `:= null`"),
                "Annotate explicitly, e.g. `var name: Node = null`",
            );
        }
        return;
    }
    if let Some(after_colon) = rest.strip_prefix(':') {
        let type_part = after_colon.split('=').next().unwrap_or("").trim();
        if type_part.is_empty() {
            push_typing_diag(
                ctx,
                line,
                col,
                format!("Variable `{var_name}` has an empty type annotation"),
                "Specify a concrete type after `:`",
            );
        }
        return;
    }
    push_typing_diag(
        ctx,
        line,
        col,
        format!("Variable `{var_name}` is missing a static type annotation"),
        "Add `: Type` or `:= expr` (except `:= null`)",
    );
}

fn check_func_typing(ctx: &mut GdContext<'_>, start_idx: usize) -> usize {
    let func_line = ctx.lines[start_idx].line;
    let (params, ret_tail, next_idx) = collect_func_header(ctx.lines, start_idx);
    let func_allowed =
        has_godot_allow(&ctx.comments, func_line, "godot4-static-typing", ctx.config);

    if !func_allowed {
        for (param_line, param_text) in params {
            if !has_godot_allow(&ctx.comments, param_line, "godot4-static-typing", ctx.config)
                && !param_has_type(&param_text)
            {
                let name = param_text.split('=').next().unwrap_or(&param_text).trim();
                push_typing_diag(
                    ctx,
                    param_line,
                    1,
                    format!("Parameter `{name}` is missing a `: Type` annotation"),
                    "Annotate every function parameter with `: Type`",
                );
            }
        }
        if !has_return_type(&ret_tail) {
            push_typing_diag(
                ctx,
                func_line,
                1,
                "Function is missing an explicit `-> ReturnType` annotation".to_owned(),
                "Add `-> void` or the concrete return type before `:`",
            );
        }
    }

    next_idx
}

struct FuncHeaderAcc {
    params: Vec<(u32, String)>,
    ret_tail: String,
    cur_param: String,
    cur_line: u32,
    state: HeaderParseState,
    depth: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderParseState {
    BeforeOpenParen,
    InParams,
    AfterCloseParen,
}

impl FuncHeaderAcc {
    fn new(start_line: u32) -> Self {
        Self {
            params: Vec::new(),
            ret_tail: String::new(),
            cur_param: String::new(),
            cur_line: start_line,
            state: HeaderParseState::BeforeOpenParen,
            depth: 0,
        }
    }

    fn feed_line(&mut self, line_num: u32, code: &str) -> bool {
        for b in code.bytes() {
            match self.state {
                HeaderParseState::BeforeOpenParen if b == b'(' => {
                    self.state = HeaderParseState::InParams;
                    self.depth = 1;
                    self.cur_line = line_num;
                }
                HeaderParseState::InParams => self.feed_param_byte(line_num, b),
                HeaderParseState::AfterCloseParen if b == b':' => return true,
                HeaderParseState::AfterCloseParen => self.ret_tail.push(b as char),
                HeaderParseState::BeforeOpenParen => {}
            }
        }
        self.state == HeaderParseState::AfterCloseParen && !code.trim_end().ends_with('\\')
    }

    fn feed_param_byte(&mut self, line_num: u32, b: u8) {
        match b {
            b'(' | b'[' | b'{' => {
                self.depth += 1;
                self.cur_param.push(b as char);
            }
            b')' | b']' | b'}' => {
                self.depth -= 1;
                if self.depth == 0 && b == b')' {
                    push_param_if_nonempty(&mut self.params, self.cur_line, &mut self.cur_param);
                    self.state = HeaderParseState::AfterCloseParen;
                } else {
                    self.cur_param.push(b as char);
                }
            }
            b',' if self.depth == 1 => {
                push_param_if_nonempty(&mut self.params, self.cur_line, &mut self.cur_param);
                self.cur_line = line_num;
            }
            _ => {
                if self.cur_param.trim().is_empty() {
                    self.cur_line = line_num;
                }
                self.cur_param.push(b as char);
            }
        }
    }
}

fn collect_func_header(
    lines: &[GdLine<'_>],
    start_idx: usize,
) -> (Vec<(u32, String)>, String, usize) {
    let mut acc = FuncHeaderAcc::new(lines[start_idx].line);
    let mut idx = start_idx;
    while idx < lines.len() {
        let line = &lines[idx];
        if !line.starts_in_str && acc.feed_line(line.line, &line.code) {
            return (acc.params, acc.ret_tail, idx + 1);
        }
        idx += 1;
    }
    (acc.params, acc.ret_tail, idx)
}

fn push_param_if_nonempty(params: &mut Vec<(u32, String)>, line: u32, buf: &mut String) {
    let trimmed = buf.trim().to_owned();
    buf.clear();
    if !trimmed.is_empty() {
        params.push((line, trimmed));
    }
}

fn param_has_type(param: &str) -> bool {
    let mut depth = 0usize;
    let mut before_eq = param;
    for (idx, b) in param.bytes().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 => {
                before_eq = &param[..idx];
                break;
            }
            _ => {}
        }
    }
    let Some((name, ty)) = before_eq.split_once(':') else {
        return false;
    };
    !name.trim().is_empty() && !ty.trim().is_empty()
}

fn has_return_type(ret_tail: &str) -> bool {
    let Some((_, after)) = ret_tail.split_once("->") else {
        return false;
    };
    !after.trim().is_empty()
}

fn push_typing_diag(
    ctx: &mut GdContext<'_>,
    line: u32,
    col: u32,
    message: String,
    suggestion: &str,
) {
    push_diag(
        &mut ctx.diagnostics,
        "godot4-static-typing",
        Severity::Error,
        ctx.file,
        (line, col),
        message,
        Some(suggestion),
    );
}

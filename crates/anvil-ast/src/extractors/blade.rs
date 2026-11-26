//! AST and directive edge extractor for Laravel Blade (`.blade.php`) templates.

use std::sync::OnceLock;

use anvil_core::traits::EdgeKind;
use regex::Regex;
use tree_sitter::Node;

use super::ExtractState;

/// Visits the root of a Blade template once to extract `@extends`, `@include*`, `@component`,
/// `@livewire`, `<x-...>` (including dotted `<x-ui.button>`), and `<livewire:...>` tags.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    if node.parent().is_none() {
        extract_blade_references(state);
    }
}

fn extract_blade_references(state: &mut ExtractState<'_>) {
    static VIEW_DIRECTIVE_RE: OnceLock<Regex> = OnceLock::new();
    static INCLUDE_COND_RE: OnceLock<Regex> = OnceLock::new();
    static LIVEWIRE_DIRECTIVE_RE: OnceLock<Regex> = OnceLock::new();
    static X_COMPONENT_RE: OnceLock<Regex> = OnceLock::new();
    static LIVEWIRE_TAG_RE: OnceLock<Regex> = OnceLock::new();

    let view_re = VIEW_DIRECTIVE_RE.get_or_init(|| {
        Regex::new(
            r"@(?:extends|include|includeIf|each|component)\s*\(\s*['\x22]([^'\x22]+)['\x22]",
        )
        .expect("valid blade view directive regex")
    });
    let cond_re = INCLUDE_COND_RE.get_or_init(|| {
        Regex::new(r"@(?:includeWhen|includeUnless)\s*\([^,]+,\s*['\x22]([^'\x22]+)['\x22]")
            .expect("valid blade conditional include regex")
    });
    let lw_re = LIVEWIRE_DIRECTIVE_RE.get_or_init(|| {
        Regex::new(r"@livewire\s*\(\s*['\x22]([^'\x22]+)['\x22]")
            .expect("valid livewire directive regex")
    });
    let x_tag_re = X_COMPONENT_RE.get_or_init(|| {
        Regex::new(r"<x-([A-Za-z0-9][A-Za-z0-9_.:-]*)").expect("valid blade x-component regex")
    });
    let lw_tag_re = LIVEWIRE_TAG_RE.get_or_init(|| {
        Regex::new(r"<livewire:([A-Za-z0-9][A-Za-z0-9_.-]*)").expect("valid livewire tag regex")
    });

    for (idx, line_text) in state.source.lines().enumerate() {
        let line = (idx + 1) as u32;
        push_matches(view_re, line_text, "blade", line, state);
        push_matches(cond_re, line_text, "blade", line, state);
        push_matches(lw_re, line_text, "livewire", line, state);
        push_matches(x_tag_re, line_text, "blade_component", line, state);
        push_matches(lw_tag_re, line_text, "livewire", line, state);
    }
}

fn push_matches(
    re: &Regex,
    line_text: &str,
    scheme: &str,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    for caps in re.captures_iter(line_text) {
        if let Some(m) = caps.get(1) {
            let target = m.as_str().trim();
            if !target.is_empty() {
                state.push_edge(
                    None,
                    format!("{scheme}:{target}"),
                    EdgeKind::RendersTemplate,
                    line,
                );
            }
        }
    }
}

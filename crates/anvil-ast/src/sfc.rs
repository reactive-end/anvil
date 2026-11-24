//! Single-File Component (`.vue`, `.svelte`, `.astro`) block extraction and analysis.

use std::path::Path;

use anyhow::{Context, Result};
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{Diagnostic, FileAstFacts, Language};
use tree_sitter::Node;

use crate::rules::{self, RuleState};
use crate::{extractors, parser};

struct SfcBlock {
    lang: Language,
    content_start: usize,
    content_end: usize,
    mask_start: usize,
    mask_end: usize,
}

/// Returns `true` for `.vue`, `.svelte`, and `.astro` paths (case-insensitive ASCII).
pub fn is_sfc_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
        ext.eq_ignore_ascii_case("vue")
            || ext.eq_ignore_ascii_case("svelte")
            || ext.eq_ignore_ascii_case("astro")
    })
}

/// Analyzes a `.vue`, `.svelte`, or `.astro` SFC by extracting embedded script, style,
/// frontmatter, and template comment regions while preserving exact 1-based line and column coordinates.
pub fn analyze_sfc(
    path: &Path,
    normalized_file: &str,
    source: &str,
    blake3_hash: String,
    config: &AnvilConfig,
) -> Result<FileAstFacts> {
    let is_astro = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("astro"));
    let blocks = extract_blocks(source, is_astro);
    let file_language = resolve_sfc_language(&blocks, is_astro);

    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut diagnostics = Vec::new();

    for block in &blocks {
        let padded = build_padded_block(source, block.content_start, block.content_end);
        let tree = parser::parse_source(block.lang, &padded)?;
        let facts = extractors::collect_file_facts(
            path,
            normalized_file,
            &padded,
            String::new(),
            block.lang,
            tree.as_ref(),
            config,
        );
        symbols.extend(facts.symbols);
        edges.extend(facts.edges);
        diagnostics.extend(
            facts
                .diagnostics
                .into_iter()
                .filter(|d| d.rule != "root-hygiene"),
        );
    }

    collect_template_comment_diagnostics(
        path,
        normalized_file,
        source,
        &blocks,
        config,
        &mut diagnostics,
    )?;

    if let Some(diag) = rules::root_hygiene::check_root_hygiene(path, normalized_file, config) {
        diagnostics.push(diag);
    }

    Ok(FileAstFacts {
        path: normalized_file.to_owned(),
        blake3_hash,
        language: file_language,
        symbols,
        edges,
        diagnostics,
    })
}

fn resolve_sfc_language(blocks: &[SfcBlock], is_astro: bool) -> Language {
    let has_ts = is_astro
        || blocks
            .iter()
            .any(|b| matches!(b.lang, Language::TypeScript | Language::Tsx));
    if has_ts {
        Language::TypeScript
    } else {
        Language::JavaScript
    }
}

fn extract_blocks(source: &str, is_astro: bool) -> Vec<SfcBlock> {
    let mut blocks = Vec::new();
    let mut i = 0usize;

    if is_astro && let Some((fm, next_pos)) = extract_astro_frontmatter(source) {
        blocks.push(fm);
        i = next_pos;
    }

    let bytes = source.as_bytes();
    while i < bytes.len() {
        if source[i..].starts_with("<!--") {
            i = source[i + 4..]
                .find("-->")
                .map_or(bytes.len(), |rel| i + 4 + rel + 3);
            continue;
        }
        if bytes[i] == b'<'
            && let Some((block_opt, next_pos)) = parse_sfc_tag(source, i, is_astro)
        {
            if let Some(block) = block_opt {
                blocks.push(block);
            }
            i = next_pos;
            continue;
        }
        i += 1;
    }

    blocks
}

fn extract_astro_frontmatter(source: &str) -> Option<(SfcBlock, usize)> {
    let first_nl = source.find('\n')?;
    let first_line = source[..first_nl]
        .trim_end_matches('\r')
        .trim_end_matches([' ', '\t']);
    if first_line != "---" {
        return None;
    }

    let fm_start = first_nl + 1;
    let mut pos = fm_start;
    while pos <= source.len() {
        let line_end = source[pos..]
            .find('\n')
            .map_or(source.len(), |rel| pos + rel);
        let line = source[pos..line_end]
            .trim_end_matches('\r')
            .trim_end_matches([' ', '\t']);
        if line == "---" {
            let mask_end = if line_end < source.len() {
                line_end + 1
            } else {
                line_end
            };
            let block = SfcBlock {
                lang: Language::TypeScript,
                content_start: fm_start,
                content_end: pos,
                mask_start: 0,
                mask_end,
            };
            return Some((block, mask_end));
        }
        if line_end >= source.len() {
            break;
        }
        pos = line_end + 1;
    }
    None
}

fn parse_sfc_tag(
    source: &str,
    tag_start: usize,
    is_astro: bool,
) -> Option<(Option<SfcBlock>, usize)> {
    let (is_script, prefix_len) = match_tag_prefix(&source[tag_start..])?;
    let after_name = *source.as_bytes().get(tag_start + prefix_len)?;
    if !after_name.is_ascii_whitespace() && after_name != b'>' && after_name != b'/' {
        return None;
    }

    let gt_pos = find_open_tag_end(source, tag_start + prefix_len)?;
    let attrs = &source[tag_start + prefix_len..gt_pos];
    if attrs.trim_end().ends_with('/') {
        return Some((None, gt_pos + 1));
    }

    let content_start = gt_pos + 1;
    let (content_end, close_end) = find_close_tag(source, content_start, is_script)?;
    let lang = resolve_block_language(is_script, attrs, is_astro);

    Some((
        Some(SfcBlock {
            lang,
            content_start,
            content_end,
            mask_start: tag_start,
            mask_end: close_end,
        }),
        close_end,
    ))
}

fn match_tag_prefix(slice: &str) -> Option<(bool, usize)> {
    if slice.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("<script")) {
        Some((true, 7))
    } else if slice.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("<style")) {
        Some((false, 6))
    } else {
        None
    }
}

fn find_open_tag_end(source: &str, start: usize) -> Option<usize> {
    let mut in_quote: Option<u8> = None;
    for (idx, b) in source[start..].bytes().enumerate() {
        match in_quote {
            Some(q) if b == q => in_quote = None,
            None if b == b'"' || b == b'\'' => in_quote = Some(b),
            None if b == b'>' => return Some(start + idx),
            _ => {}
        }
    }
    None
}

fn find_close_tag(source: &str, start: usize, is_script: bool) -> Option<(usize, usize)> {
    let target = if is_script { "</script" } else { "</style" };
    let target_len = target.len();
    let mut pos = start;

    while let Some(rel) = source[pos..].find('<') {
        let lt_pos = pos + rel;
        let tail = &source[lt_pos..];
        if tail
            .get(..target_len)
            .is_some_and(|p| p.eq_ignore_ascii_case(target))
        {
            let mut cursor = lt_pos + target_len;
            let bytes = source.as_bytes();
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b'>' {
                return Some((lt_pos, cursor + 1));
            }
        }
        pos = lt_pos + 1;
    }
    None
}

fn resolve_block_language(is_script: bool, attrs: &str, is_astro: bool) -> Language {
    let lang_val = extract_attr(attrs, "lang").unwrap_or("").trim();
    if is_script {
        if lang_val.eq_ignore_ascii_case("tsx") {
            Language::Tsx
        } else if lang_val.eq_ignore_ascii_case("ts")
            || lang_val.eq_ignore_ascii_case("typescript")
            || is_astro
        {
            Language::TypeScript
        } else if lang_val.eq_ignore_ascii_case("jsx") {
            Language::Jsx
        } else {
            Language::JavaScript
        }
    } else if lang_val.eq_ignore_ascii_case("scss") {
        Language::Scss
    } else {
        Language::Css
    }
}

fn extract_attr<'a>(attrs: &'a str, target: &str) -> Option<&'a str> {
    let bytes = attrs.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'/'
        {
            i += 1;
        }
        let name = &attrs[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            if i < bytes.len() && bytes[i] == b'/' {
                i += 1;
            }
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let val = parse_attr_value(attrs, &mut i);
        if name.eq_ignore_ascii_case(target) {
            return Some(val);
        }
    }
    None
}

fn parse_attr_value<'a>(attrs: &'a str, idx: &mut usize) -> &'a str {
    let bytes = attrs.as_bytes();
    if *idx >= bytes.len() {
        return "";
    }
    let quote = bytes[*idx];
    if quote == b'"' || quote == b'\'' {
        *idx += 1;
        let start = *idx;
        while *idx < bytes.len() && bytes[*idx] != quote {
            *idx += 1;
        }
        let val = &attrs[start..*idx];
        if *idx < bytes.len() {
            *idx += 1;
        }
        return val;
    }
    let start = *idx;
    while *idx < bytes.len() && !bytes[*idx].is_ascii_whitespace() {
        *idx += 1;
    }
    &attrs[start..*idx]
}

fn build_padded_block(source: &str, content_start: usize, content_end: usize) -> String {
    let prefix = &source[..content_start];
    let newlines = prefix.bytes().filter(|&b| b == b'\n').count();
    let col_bytes = match prefix.rfind('\n') {
        Some(pos) => content_start - (pos + 1),
        None => content_start,
    };
    let content = &source[content_start..content_end];
    let mut padded = String::with_capacity(newlines + col_bytes + content.len());
    for _ in 0..newlines {
        padded.push('\n');
    }
    for _ in 0..col_bytes {
        padded.push(' ');
    }
    padded.push_str(content);
    padded
}

fn mask_blocks(source: &str, blocks: &[SfcBlock]) -> Result<String> {
    let mut bytes = source.as_bytes().to_vec();
    for block in blocks {
        for b in &mut bytes[block.mask_start..block.mask_end] {
            if *b != b'\n' && *b != b'\r' {
                *b = b' ';
            }
        }
    }
    String::from_utf8(bytes).context("SFC template masking produced invalid UTF-8")
}

fn collect_template_comment_diagnostics(
    path: &Path,
    normalized_file: &str,
    source: &str,
    blocks: &[SfcBlock],
    config: &AnvilConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let masked = mask_blocks(source, blocks)?;
    let Some(tree) = parser::parse_source(Language::Html, &masked)? else {
        return Ok(());
    };
    let root = tree.root_node();
    let mut state = RuleState::new(
        normalized_file,
        path,
        &masked,
        Language::Html,
        config,
        Some(root),
    );
    walk_html_comments(root, &mut state);
    diagnostics.extend(state.diagnostics);
    Ok(())
}

fn walk_html_comments(root: Node<'_>, state: &mut RuleState<'_>) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if rules::is_comment_kind(node.kind()) {
            rules::check_node(node, state);
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

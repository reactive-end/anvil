//! Tier 0 rule: block error handlers that drop the error without a trace.
//!
//! Flags an empty (or comment-only) `catch` body in TypeScript/JavaScript, PHP, Kotlin, and Dart,
//! a `.catch(() => {})` promise handler, a Python `except` whose body is only `pass` or `...`, and
//! a Rust `Err(..) => {}` / `Err(..) => ()` match arm. In TypeScript/JavaScript a `catch` whose
//! only statement returns a constant (`return 0`, `return []`, `return null`) without looking at
//! the error is flagged too: the failure becomes a normal-looking value. A handler that logs,
//! rethrows, builds its fallback from the error, or does anything else is left alone.

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow, is_comment_kind};

/// Rule id; reported only on lines the pending change adds (see `rules::retain_new_code_findings`).
pub const RULE: &str = "no-swallowed-errors";

/// Evaluates the node during the single AST walk.
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    let swallowed = match state.language {
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
            (node.kind() == "catch_clause"
                && (field_is_empty(node, "body") || is_silent_fallback(node, state.source)))
                || is_empty_promise_catch(node, state.source)
        }
        Language::Php => node.kind() == "catch_clause" && field_is_empty(node, "body"),
        Language::Kotlin => node.kind() == "catch_block" && last_block_is_empty(node),
        Language::Dart => is_empty_dart_handler(node),
        Language::Python => node.kind() == "except_clause" && python_body_only_passes(node),
        Language::Rust => node.kind() == "match_arm" && is_ignored_err_arm(node, state.source),
        _ => false,
    };
    if !swallowed {
        return;
    }
    let line = node.start_position().row as u32 + 1;
    if has_anvil_allow(state, line, RULE) {
        return;
    }
    state.diagnostics.push(Diagnostic {
        rule: RULE.to_owned(),
        severity: Severity::Error,
        file: state.file.to_owned(),
        line,
        column: node.start_position().column as u32 + 1,
        message: "Error handler drops the error silently".to_owned(),
        suggestion: Some(
            "Handle it (recover, return a fallback), log it, or let it propagate; if ignoring is the contract, catch the narrowest error and add `anvil-allow(no-swallowed-errors): <reason> [<ref>]`"
                .to_owned(),
        ),
    });
}

/// `true` when the block has no statements, comments aside.
fn is_empty_block(block: Node<'_>) -> bool {
    let mut cursor = block.walk();
    block.named_children(&mut cursor).all(|child| is_comment_kind(child.kind()))
}

fn field_is_empty(node: Node<'_>, field: &str) -> bool {
    node.child_by_field_name(field).is_some_and(is_empty_block)
}

/// Constant results a handler can hand back without saying anything went wrong.
const CONSTANT_KINDS: &[&str] =
    &["number", "string", "true", "false", "null", "undefined", "array", "object"];

/// `catch (e) { return 0; }`: the only statement returns a constant (or nothing) and the caught
/// error is never looked at, so the failure turns into a normal-looking value with no trace.
fn is_silent_fallback(node: Node<'_>, source: &str) -> bool {
    let Some(body) = node.child_by_field_name("body") else {
        return false;
    };
    let mut cursor = body.walk();
    let statements: Vec<Node<'_>> =
        body.named_children(&mut cursor).filter(|c| !is_comment_kind(c.kind())).collect();
    let [only] = statements.as_slice() else {
        return false;
    };
    let returns_constant = only.kind() == "return_statement"
        && only.named_child(0).is_none_or(|value| {
            CONSTANT_KINDS.contains(&value.kind()) && value.named_child_count() == 0
        });
    let error_used = node
        .child_by_field_name("parameter")
        .and_then(|p| p.utf8_text(source.as_bytes()).ok())
        .is_some_and(|name| {
            let text = body.utf8_text(source.as_bytes()).unwrap_or_default();
            text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).any(|w| w == name)
        });
    returns_constant && !error_used
}

fn last_block_is_empty(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let block = node.named_children(&mut cursor).filter(|c| c.kind() == "block").last();
    block.is_some_and(is_empty_block)
}

/// Dart lists each handler's block directly under `try_statement`, after its `on` type or `catch`
/// part; the protected body is the `body` field and `finally` wraps its own block.
fn is_empty_dart_handler(node: Node<'_>) -> bool {
    node.kind() == "block"
        && node.parent().is_some_and(|parent| {
            parent.kind() == "try_statement"
                && parent.child_by_field_name("body").is_none_or(|body| body.id() != node.id())
        })
        && is_empty_block(node)
}

/// `promise.catch(() => {})` or `.catch(function () {})`.
fn is_empty_promise_catch(node: Node<'_>, source: &str) -> bool {
    if node.kind() != "call_expression" {
        return false;
    }
    let is_catch = node
        .child_by_field_name("function")
        .filter(|callee| callee.kind() == "member_expression")
        .and_then(|callee| callee.child_by_field_name("property"))
        .is_some_and(|property| property.utf8_text(source.as_bytes()) == Ok("catch"));
    if !is_catch {
        return false;
    }
    let Some(args) = node.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = args.walk();
    let handler = args.named_children(&mut cursor).find(|arg| !is_comment_kind(arg.kind()));
    handler.is_some_and(|handler| {
        matches!(handler.kind(), "arrow_function" | "function_expression" | "function")
            && handler
                .child_by_field_name("body")
                .is_some_and(|body| body.kind() == "statement_block" && is_empty_block(body))
    })
}

fn python_body_only_passes(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let Some(block) = node.named_children(&mut cursor).find(|c| c.kind() == "block") else {
        return false;
    };
    let mut inner = block.walk();
    let statements: Vec<Node<'_>> =
        block.named_children(&mut inner).filter(|c| !is_comment_kind(c.kind())).collect();
    !statements.is_empty() && statements.iter().all(|s| is_python_noop(*s))
}

fn is_python_noop(statement: Node<'_>) -> bool {
    statement.kind() == "pass_statement"
        || (statement.kind() == "expression_statement"
            && statement.named_child(0).is_some_and(|e| e.kind() == "ellipsis"))
}

/// `Err(..) => {}` or `Err(..) => ()`.
fn is_ignored_err_arm(node: Node<'_>, source: &str) -> bool {
    let Some(pattern) = node.child_by_field_name("pattern") else {
        return false;
    };
    let pattern_text = pattern.utf8_text(source.as_bytes()).unwrap_or_default().trim();
    let is_err = pattern_text.starts_with("Err(") || pattern_text.starts_with("Result::Err(");
    let ignored = node.child_by_field_name("value").is_some_and(|value| match value.kind() {
        "block" => is_empty_block(value),
        "unit_expression" => true,
        _ => false,
    });
    is_err && !pattern_text.contains(" if ") && ignored
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anvil_core::config::AnvilConfig;

    use crate::analyze_file;

    fn rule_lines(path: &str, source: &str) -> Vec<u32> {
        let facts = analyze_file(Path::new(path), source, &AnvilConfig::default()).expect("parse");
        facts.diagnostics.iter().filter(|d| d.rule == super::RULE).map(|d| d.line).collect()
    }

    #[test]
    fn typescript_empty_catch_and_empty_promise_catch() {
        let src = "try {\n  run();\n} catch (e) {}\ntry {\n  run();\n} catch {\n  // ignore\n}\nload().catch(() => {});\nload().catch((e) => { log(e); });\ntry {\n  run();\n} catch (e) {\n  throw new Wrapped(e);\n}\n";
        assert_eq!(rule_lines("src/a.ts", src), vec![3, 6, 9]);
        assert_eq!(rule_lines("src/a.js", "try { a(); } catch (e) {}\n"), vec![1]);
    }

    #[test]
    fn typescript_catch_that_only_returns_a_constant_is_flagged() {
        let src = "function a() {\n  try {\n    return run();\n  } catch (e) {\n    return 0;\n  }\n}\nfunction b() {\n  try {\n    return run();\n  } catch {\n    return [];\n  }\n}\nfunction c() {\n  try {\n    return run();\n  } catch (e) {\n    return fallbackFor(e);\n  }\n}\nfunction d() {\n  try {\n    return run();\n  } catch (e) {\n    log(e);\n    return 0;\n  }\n}\nfunction f() {\n  try {\n    return run();\n  } catch (err) {\n    return [err.message];\n  }\n}\n";
        assert_eq!(rule_lines("src/a.ts", src), vec![4, 11]);
    }

    #[test]
    fn python_except_pass_but_not_handled_except() {
        let src = "try:\n    run()\nexcept Exception:\n    pass\ntry:\n    run()\nexcept:\n    ...\ntry:\n    run()\nexcept OSError as err:\n    log(err)\n";
        assert_eq!(rule_lines("app/a.py", src), vec![3, 7]);
    }

    #[test]
    fn php_kotlin_and_dart_empty_catch() {
        let php = "<?php\ndeclare(strict_types=1);\ntry { run(); } catch (Exception $e) {}\ntry { run(); } catch (Exception $e) { report($e); }\n";
        assert_eq!(rule_lines("app/A.php", php), vec![3]);
        let kotlin = "fun a() {\n    try { run() } catch (e: Exception) {}\n    try { run() } catch (e: Exception) { log(e) }\n}\n";
        assert_eq!(rule_lines("src/A.kt", kotlin), vec![2]);
        let dart = "void a() {\n  try { run(); } catch (e) {}\n  try { run(); } on FormatException {}\n  try { run(); } catch (e) { log(e); }\n  try { run(); } finally {}\n}\n";
        assert_eq!(rule_lines("lib/a.dart", dart), vec![2, 3]);
    }

    #[test]
    fn rust_ignored_err_arm_but_not_handled_or_guarded() {
        let src = "fn a() {\n    match run() {\n        Ok(v) => use_it(v),\n        Err(_) => {}\n    }\n    match run() {\n        Ok(_) => (),\n        Err(e) => (),\n    }\n    match run() {\n        Ok(_) => {}\n        Err(e) => log(e),\n    }\n}\n";
        assert_eq!(rule_lines("src/a.rs", src), vec![4, 8]);
    }

    #[test]
    fn structured_allow_accepts_an_intentional_ignore() {
        let mut config = AnvilConfig::default();
        config.allowed_escapes.push("anvil-allow".to_owned());
        let src = "try {\n  run();\n  // anvil-allow(no-swallowed-errors): cleanup is best effort [OPS-7]\n} catch (e) {}\n";
        let facts = crate::analyze_file(Path::new("src/a.ts"), src, &config).expect("parse");
        assert!(facts.diagnostics.iter().all(|d| d.rule != super::RULE), "{:?}", facts.diagnostics);
    }

    #[test]
    fn only_findings_on_added_lines_survive_the_new_code_filter() {
        use anvil_core::git::AddedLines;
        use anvil_core::traits::{Diagnostic, Severity};

        let finding = |rule: &str, line: u32| Diagnostic {
            rule: rule.to_owned(),
            severity: Severity::Error,
            file: "src/a.ts".to_owned(),
            line,
            column: 1,
            message: String::new(),
            suggestion: None,
        };
        let all =
            vec![finding(super::RULE, 3), finding(super::RULE, 9), finding("no-todo-comments", 9)];
        let mut added = AddedLines::default();
        added.insert("src/a.ts".to_owned(), vec![(2, 4)]);

        let mut kept = all.clone();
        crate::rules::retain_new_code_findings(r"src\a.ts", &mut kept, Some(&added));
        let lines: Vec<(String, u32)> = kept.iter().map(|d| (d.rule.clone(), d.line)).collect();
        assert_eq!(lines, vec![(super::RULE.to_owned(), 3), ("no-todo-comments".to_owned(), 9)]);

        let mut unknown = all.clone();
        crate::rules::retain_new_code_findings("src/a.ts", &mut unknown, None);
        assert_eq!(unknown.len(), 3, "unknown added lines keep every finding");
    }
}

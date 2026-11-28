//! `anvil prompt improve`: turns a rough request into a structured prompt (goal, context,
//! constraints, acceptance criteria, verification, open questions) built from this repository's
//! own graph, docs, memory, and configuration. No model is involved.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use anvil_core::config::AnvilConfig;
use anvil_core::diff_classifier::{ChangedFile, classify_files};
use anvil_graph::impact::compute_blast_radius;
use anyhow::{Result, bail};
use serde::Serialize;

use super::constraints::constraints;
use super::gaps::{self, Gap};
use super::gather::{self, Limits};
use super::refs::{CodeRef, DocRef, Gathered, MemoryRef};
use super::terms;
use super::verify::QaTargets;

const LIMITS: Limits =
    Limits { code: 8, docs: 4, memories: 4, time: Duration::from_secs(2), grep: true };
/// Related tests listed; the graph can know hundreds.
const MAX_TESTS: usize = 3;
/// Flows named one per line before the rest are summarised.
const MAX_FLOW_LINES: usize = 5;
/// Files whose related tests are looked up.
const MAX_TEST_FILES: usize = 4;

/// The structured prompt.
#[derive(Debug, Serialize)]
pub struct Improved {
    pub goal: String,
    pub context: Context,
    pub constraints: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub verification: Vec<String>,
    pub open_questions: Vec<String>,
    pub gaps: Vec<Gap>,
    /// Milliseconds the graph, docs, and memory lookups took.
    pub gather_ms: u64,
}

/// Items found for the goal.
#[derive(Debug, Serialize)]
pub struct Context {
    pub code: Vec<CodeRef>,
    pub docs: Vec<DocRef>,
    pub memory: Vec<MemoryRef>,
}

/// Runs `anvil prompt improve [TEXT] [--json]`.
pub fn run(root: &Path, text: Option<String>, json: bool) -> Result<()> {
    let goal = read_goal(text)?;
    let config = AnvilConfig::load_or_default(root)?;
    let (improved, problems) = build(root, &config, goal);
    for problem in &problems {
        eprintln!("note: {problem}");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&improved)?);
    } else {
        print!("{}", markdown(&improved));
    }
    Ok(())
}

fn read_goal(text: Option<String>) -> Result<String> {
    let goal = match text.as_deref() {
        Some(t) if t != "-" => t.to_owned(),
        _ => {
            let mut raw = String::new();
            std::io::stdin().read_to_string(&mut raw)?;
            raw.trim_end_matches(['\r', '\n']).to_owned()
        }
    };
    if goal.trim().is_empty() {
        bail!("no prompt text: pass it as an argument or on stdin");
    }
    Ok(goal)
}

/// Builds the structured prompt; the second value lists lookups that failed.
pub fn build(root: &Path, config: &AnvilConfig, goal: String) -> (Improved, Vec<String>) {
    let found = terms::extract(&goal);
    let mut gathered = gather::gather(root, config, &found, LIMITS);
    let mut problems = std::mem::take(&mut gathered.problems);
    let qa = QaTargets::load(root, config, &mut problems);
    let gaps = gaps::detect(&goal, &gathered);
    let files = context_files(&gathered);
    let gather_ms = gathered.elapsed_ms;
    let tests = related_tests(root, &files, &mut problems);
    let improved = Improved {
        acceptance_criteria: acceptance(config, &qa, &files, &tests),
        verification: verification(&qa, &tests),
        open_questions: gaps.iter().map(|g| g.question().to_owned()).collect(),
        constraints: constraints(config),
        context: Context { code: gathered.code, docs: gathered.docs, memory: gathered.memories },
        gaps,
        goal,
        gather_ms,
    };
    (improved, problems)
}

/// Files of the resolved code items, without repeats.
fn context_files(gathered: &Gathered) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for code in &gathered.code {
        if !files.contains(&code.file) {
            files.push(code.file.clone());
        }
    }
    files
}

/// Test files the graph links to `files`; empty without a graph.
fn related_tests(root: &Path, files: &[String], problems: &mut Vec<String>) -> Vec<String> {
    if files.is_empty() {
        return Vec::new();
    }
    let graph = gather::load_graph(root, problems);
    let Some(graph) = graph else {
        return Vec::new();
    };
    let wanted: Vec<String> = files.iter().take(MAX_TEST_FILES).cloned().collect();
    compute_blast_radius(&graph, &wanted, 2).related_tests
}

fn acceptance(
    config: &AnvilConfig,
    qa: &QaTargets,
    files: &[String],
    tests: &[String],
) -> Vec<String> {
    let mut items = vec!["[ ] `anvil check` passes (lint, types, architecture rules)".to_owned()];
    if !qa.scenarios.is_empty() && config.qa.api.touches(files) {
        items.push("[ ] API scenarios pass: `anvil qa api`".to_owned());
    }
    let ui_touched = classify_files(&files.iter().map(ChangedFile::from_path).collect::<Vec<_>>())
        .needs_visual_qa;
    if !qa.flows.is_empty() && ui_touched {
        items.push("[ ] Web flows pass: `anvil qa run --all-flows`".to_owned());
    }
    if !tests.is_empty() {
        items.push(format!(
            "[ ] The {} related test file(s) pass (listed under Verification)",
            tests.len()
        ));
    }
    items.push("[ ] <describe what done looks like>".to_owned());
    items.push("[ ] <describe how errors and edge cases should behave>".to_owned());
    items
}

fn verification(qa: &QaTargets, tests: &[String]) -> Vec<String> {
    let mut lines = vec![
        "`anvil check`: lint, types, architecture rules, and the tests related to the changed files"
            .to_owned(),
    ];
    if !qa.scenarios.is_empty() {
        lines.push(format!(
            "`anvil qa api`: {} API scenario(s): {} (one at a time: `anvil qa api <name>`)",
            qa.scenarios.len(),
            qa.scenarios.join(", ")
        ));
    }
    for flow in qa.flows.iter().take(MAX_FLOW_LINES) {
        lines.push(format!("`anvil qa run --flow {flow}`"));
    }
    if qa.flows.len() > MAX_FLOW_LINES {
        lines.push("`anvil qa run --all-flows`: every web flow".to_owned());
    }
    if !tests.is_empty() {
        lines.push(format!("Tests the graph links to this code: {}", listed(tests, MAX_TESTS)));
    }
    lines
}

fn listed(items: &[String], max: usize) -> String {
    let shown = items.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    match items.len().saturating_sub(max) {
        0 => shown,
        more => format!("{shown} (+{more} more)"),
    }
}

/// Renders `improved` as the Markdown the user reviews and edits.
pub fn markdown(improved: &Improved) -> String {
    let mut out = String::new();
    section(&mut out, "Goal", std::slice::from_ref(&improved.goal), false);
    let context = context_lines(&improved.context);
    section(&mut out, "Context", &context, true);
    section(&mut out, "Constraints", &improved.constraints, true);
    let criteria: Vec<String> =
        improved.acceptance_criteria.iter().map(|c| format!("- {c}")).collect();
    section(&mut out, "Acceptance criteria", &criteria, false);
    section(&mut out, "Verification", &improved.verification, true);
    let questions = if improved.open_questions.is_empty() {
        vec!["None detected.".to_owned()]
    } else {
        improved.open_questions.clone()
    };
    section(&mut out, "Open questions", &questions, true);
    out
}

fn section(out: &mut String, title: &str, lines: &[String], bullets: bool) {
    if !out.is_empty() {
        out.push('\n');
    }
    let _ = writeln!(out, "## {title}");
    for line in lines {
        let _ = writeln!(out, "{}{line}", if bullets { "- " } else { "" });
    }
}

fn context_lines(context: &Context) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.extend(context.code.iter().map(|c| format!("code: `{}` ({})", c.name, location(c))));
    lines.extend(context.docs.iter().map(|d| format!("docs: {}", d.render())));
    lines.extend(context.memory.iter().map(|m| format!("memory: {}", m.render())));
    if lines.is_empty() {
        lines.push("Nothing relevant found: name the file or function you mean.".to_owned());
    }
    lines
}

fn location(code: &CodeRef) -> String {
    match code.line {
        Some(line) => format!("{} {}:{line}", code.kind, code.file),
        None => code.kind.clone() + " " + &code.file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Improved {
        Improved {
            goal: "add a discount when quantity is over 10".into(),
            context: Context {
                code: vec![CodeRef {
                    name: "Order".into(),
                    kind: "class".into(),
                    file: "orders-api/src/orders/domain/order.ts".into(),
                    line: Some(12),
                }],
                docs: Vec::new(),
                memory: Vec::new(),
            },
            constraints: vec!["Architecture: hexagonal".into()],
            acceptance_criteria: vec![
                "[ ] `anvil check` passes".into(),
                "[ ] <describe what done looks like>".into(),
            ],
            verification: vec!["`anvil check`".into()],
            open_questions: vec![Gap::NoSuccessCriteria.question().to_owned()],
            gaps: vec![Gap::NoSuccessCriteria],
            gather_ms: 0,
        }
    }

    #[test]
    fn markdown_has_every_section_in_order_with_the_goal_unchanged() {
        let md = markdown(&fixture());
        let titles = [
            "## Goal",
            "## Context",
            "## Constraints",
            "## Acceptance criteria",
            "## Verification",
            "## Open questions",
        ];
        let positions: Vec<usize> =
            titles.iter().map(|t| md.find(t).unwrap_or(usize::MAX)).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{md}");
        assert!(md.starts_with("## Goal\nadd a discount when quantity is over 10\n"), "{md}");
        assert!(
            md.contains("- code: `Order` (class orders-api/src/orders/domain/order.ts:12)"),
            "{md}"
        );
        assert!(md.contains("- [ ] <describe what done looks like>"), "{md}");
    }

    #[test]
    fn an_empty_context_says_so_instead_of_inventing_one() {
        let mut improved = fixture();
        improved.context.code.clear();
        improved.open_questions.clear();
        let md = markdown(&improved);
        assert!(
            md.contains("- Nothing relevant found: name the file or function you mean."),
            "{md}"
        );
        assert!(md.contains("## Open questions\n- None detected."), "{md}");
    }

    #[test]
    fn verification_names_each_flow_and_the_scenarios() {
        let qa = QaTargets {
            scenarios: vec!["create-order".into(), "list".into()],
            flows: vec!["checkout".into(), "browse".into()],
        };
        let lines = verification(&qa, &[]);
        assert!(
            lines[1].starts_with("`anvil qa api`: 2 API scenario(s): create-order, list"),
            "{lines:?}"
        );
        assert_eq!(lines[2], "`anvil qa run --flow checkout`");
        assert_eq!(lines[3], "`anvil qa run --flow browse`");
    }

    #[test]
    fn acceptance_derives_items_from_what_the_context_touches() {
        let mut config = AnvilConfig::default();
        config.qa.api.start_cwd = Some("orders-api".into());
        let qa = QaTargets { scenarios: vec!["a".into()], flows: vec!["f".into()] };
        let api = acceptance(&config, &qa, &["orders-api/src/order.ts".to_owned()], &[]);
        assert!(api.iter().any(|i| i.contains("`anvil qa api`")), "{api:?}");
        let ui = acceptance(&config, &qa, &["orders-web/src/App.tsx".to_owned()], &[]);
        assert!(ui.iter().any(|i| i.contains("--all-flows")), "{ui:?}");
        assert!(!ui.iter().any(|i| i.contains("`anvil qa api`")), "{ui:?}");
        assert_eq!(
            ui.last().map(String::as_str),
            Some("[ ] <describe how errors and edge cases should behave>")
        );
    }

    #[test]
    fn long_lists_are_cut_with_a_count() {
        let items: Vec<String> = (0..8).map(|n| format!("t{n}")).collect();
        assert_eq!(listed(&items, 5), "t0, t1, t2, t3, t4 (+3 more)");
        assert_eq!(listed(&items[..2], 5), "t0, t1");
    }
}

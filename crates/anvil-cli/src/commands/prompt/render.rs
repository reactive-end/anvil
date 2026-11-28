//! The text the hook adds to a prompt: the context line, the coach note for the agent, and the
//! one-line tip for the user. Everything is sized in characters (four per token).

use super::gaps::Gap;
use super::refs::Gathered;

/// Characters per estimated token.
pub const CHARS_PER_TOKEN: usize = 4;
/// `[info][prompt-coach]: ` and similar wrapping the adapter adds around each message.
const WRAPPER_CHARS: usize = 28;
/// Share of the cap (in tenths) the coach note may use, so the context line always has room.
const COACH_SHARE_TENTHS: usize = 6;
/// Verification commands named in the coach note.
const MAX_COMMANDS: usize = 3;
/// Questions asked in the coach note.
const MAX_QUESTIONS: usize = 2;

const CONTEXT_HEADER: &str = "Likely relevant (verify before relying): ";

/// Characters the whole injected block may use for `max_tokens`.
pub fn cap_chars(max_tokens: usize) -> usize {
    max_tokens.saturating_mul(CHARS_PER_TOKEN)
}

/// Renders the coach note and the context line so that together they stay within `cap` characters.
/// The coach note is built first (it is short and fixed) and the context takes what is left.
pub fn fit(gathered: &Gathered, gaps: &[Gap], commands: &[String], cap: usize) -> Fitted {
    let coach = (!gaps.is_empty()).then(|| coach_note(gaps, commands, coach_budget(cap)));
    let used = coach.as_ref().map_or(0, |c| c.chars().count() + WRAPPER_CHARS);
    let remaining = cap.saturating_sub(used + WRAPPER_CHARS);
    Fitted { context: context_line(gathered, remaining), coach }
}

/// The two texts of one prompt; either may be absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fitted {
    pub context: Option<String>,
    pub coach: Option<String>,
}

fn coach_budget(cap: usize) -> usize {
    (cap * COACH_SHARE_TENTHS / 10).saturating_sub(WRAPPER_CHARS)
}

/// `Likely relevant (...): code: A (f:1), B; docs: C; memory: D.` with as many items as fit
/// `budget` characters, taken in turn from code, docs, and memory so no kind crowds out the rest.
pub fn context_line(gathered: &Gathered, budget: usize) -> Option<String> {
    let code: Vec<String> = gathered.code.iter().map(|c| c.render()).collect();
    let docs: Vec<String> = gathered.docs.iter().map(|d| d.render()).collect();
    let memories: Vec<String> = gathered.memories.iter().map(|m| m.render()).collect();
    let (mut kept_code, mut kept_docs, mut kept_mem) = (0, 0, 0);
    let rounds = code.len().max(docs.len()).max(memories.len());
    let mut best: Option<String> = None;
    for round in 0..rounds {
        for kind in 0..3 {
            let (len, kept) = match kind {
                0 => (code.len(), &mut kept_code),
                1 => (docs.len(), &mut kept_docs),
                _ => (memories.len(), &mut kept_mem),
            };
            if round >= len {
                continue;
            }
            *kept += 1;
            let line = compose(&code[..kept_code], &docs[..kept_docs], &memories[..kept_mem]);
            if line.chars().count() > budget {
                return best;
            }
            best = Some(line);
        }
    }
    best
}

fn compose(code: &[String], docs: &[String], memories: &[String]) -> String {
    let sections: Vec<String> = [("code", code), ("docs", docs), ("memory", memories)]
        .iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(label, items)| format!("{label}: {}", items.join(", ")))
        .collect();
    format!("{CONTEXT_HEADER}{}.", sections.join("; "))
}

/// The note that tells the agent to settle the open points before it edits.
fn coach_note(gaps: &[Gap], commands: &[String], budget: usize) -> String {
    let missing: Vec<&str> = gaps.iter().map(|g| g.describe()).collect();
    let verify = commands.iter().take(MAX_COMMANDS).map(|c| format!("`{c}`")).collect::<Vec<_>>();
    let questions: Vec<&str> = gaps.iter().take(MAX_QUESTIONS).map(|g| g.question()).collect();
    let note = format!(
        "Coach: this request has {}. Before editing, restate the goal in one sentence, propose acceptance criteria and how you will verify them ({}), and ask the user to confirm: {} Do not edit until they answer.",
        missing.join("; "),
        verify.join(", "),
        questions.join(" "),
    );
    clip(&note, budget)
}

/// The one-line advice shown to the user for `gap`.
pub fn user_tip(gap: Gap) -> String {
    format!(
        "Tip: {} `anvil prompt improve \"<your request>\"` or the improve-prompt skill can structure it.",
        gap.tip()
    )
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{kept}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::prompt::refs::{CodeRef, DocRef, MemoryRef};

    fn gathered() -> Gathered {
        let code = |name: &str, line| CodeRef {
            name: name.into(),
            kind: "class".into(),
            file: format!("orders-api/src/{name}.ts"),
            line: Some(line),
        };
        Gathered {
            code: vec![code("Order", 12), code("OrderItem", 3), code("Cart", 9)],
            docs: vec![DocRef {
                id: "orders-api/orders".into(),
                title: Some("Orders module".into()),
                path: "p".into(),
                stale: true,
            }],
            memories: vec![MemoryRef {
                id: "m1".into(),
                title: "Totals use integer cents".into(),
                kind: "decision".into(),
                stale: false,
            }],
            ..Gathered::default()
        }
    }

    #[test]
    fn context_line_lists_code_docs_and_memory_with_locations() {
        let line = context_line(&gathered(), 2_000).unwrap();
        assert_eq!(
            line,
            "Likely relevant (verify before relying): code: Order (orders-api/src/Order.ts:12), OrderItem (orders-api/src/OrderItem.ts:3), Cart (orders-api/src/Cart.ts:9); docs: orders-api/orders — Orders module [stale]; memory: Totals use integer cents (decision)."
        );
    }

    #[test]
    fn truncation_keeps_whole_items_within_the_budget_and_mixes_kinds() {
        let full = context_line(&gathered(), 2_000).unwrap();
        let small = context_line(&gathered(), 160).unwrap();
        assert!(small.chars().count() <= 160, "{small}");
        assert!(small.len() < full.len());
        assert!(small.contains("code: Order ("), "{small}");
        assert!(small.contains("docs: orders-api/orders"), "docs are not crowded out: {small}");
        assert!(!small.contains("Cart"), "{small}");
    }

    #[test]
    fn nothing_fits_means_no_line() {
        assert_eq!(context_line(&gathered(), 20), None);
        assert_eq!(context_line(&Gathered::default(), 2_000), None);
    }

    #[test]
    fn the_whole_block_stays_within_the_cap() {
        let gaps = [Gap::NoTarget, Gap::NoSuccessCriteria, Gap::NoVerification];
        let commands = vec!["anvil check".to_owned(), "anvil qa api".to_owned()];
        for tokens in [60, 120, 450] {
            let cap = cap_chars(tokens);
            let fitted = fit(&gathered(), &gaps, &commands, cap);
            let total: usize = [&fitted.context, &fitted.coach]
                .iter()
                .filter_map(|t| t.as_ref())
                .map(|t| t.chars().count() + WRAPPER_CHARS)
                .sum();
            assert!(total <= cap, "{tokens} tokens: {total} > {cap}");
        }
    }

    #[test]
    fn the_coach_note_names_gaps_commands_and_questions() {
        let note = coach_note(
            &[Gap::NoTarget, Gap::NoSuccessCriteria],
            &["anvil check".to_owned(), "anvil qa api".to_owned()],
            1_000,
        );
        assert!(
            note.starts_with(
                "Coach: this request has no file, function, or doc it points at; no statement"
            ),
            "{note}"
        );
        assert!(note.contains("(`anvil check`, `anvil qa api`)"), "{note}");
        assert!(
            note.contains("Which file, function, screen, or endpoint should change?"),
            "{note}"
        );
        assert!(note.ends_with("Do not edit until they answer."), "{note}");
    }

    #[test]
    fn the_user_tip_is_one_line_in_english() {
        let tip = user_tip(Gap::NoSuccessCriteria);
        assert!(!tip.contains('\n'));
        assert!(tip.starts_with("Tip: describe what \"done\" looks like"), "{tip}");
        assert!(tip.contains("anvil prompt improve") && tip.contains("improve-prompt skill"));
    }
}

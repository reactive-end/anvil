//! What a rough request is missing: a target, success criteria, a way to verify, a sane scope.
//! The word lists are deliberately small and bilingual (English and Spanish); each is matched on
//! whole words or phrases after lowercasing and folding accents, never as a substring.

use serde::Serialize;

use super::refs::Gathered;
use super::stopwords::fold_accents;

/// Phrases that state an expected result ("should", "debe", "devuelve"), plus test words.
const SUCCESS_PHRASES: &[&str] = &[
    "should",
    "must",
    "expected",
    "expect",
    "expects",
    "returns",
    "return",
    "ensure",
    "so that",
    "make sure",
    "until",
    "test",
    "tests",
    "debe",
    "deben",
    "deberia",
    "tiene que",
    "tienen que",
    "que funcione",
    "devuelve",
    "devuelva",
    "retorna",
    "esperado",
    "espero",
    "para que",
    "asegurate",
    "hasta que",
    "prueba",
    "pruebas",
];

/// Phrases that say how the result will be checked.
const VERIFICATION_PHRASES: &[&str] = &[
    "test",
    "tests",
    "testing",
    "verify",
    "check",
    "lint",
    "build",
    "compile",
    "typecheck",
    "passes",
    "scenario",
    "qa",
    "prueba",
    "pruebas",
    "probar",
    "verifica",
    "verificar",
    "compila",
    "pasan",
    "escenario",
];

/// Words and phrases for "all of it". Bare "all" is left out on purpose: "all orders with a
/// discount" is a normal, bounded request.
const BROAD_PHRASES: &[&str] = &[
    "everything",
    "everywhere",
    "whole",
    "entire",
    "all the code",
    "all of it",
    "all files",
    "todo lo",
    "todo el",
    "toda la",
    "todo esto",
    "todo eso",
    "todos los archivos",
];

/// One thing a request lacks, in the order they are raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gap {
    /// No symbol, doc, or file the request points at.
    NoTarget,
    /// "Everything", "toda la app": too big to verify.
    BroadScope,
    /// Nothing says what the result should be.
    NoSuccessCriteria,
    /// Nothing says how it will be checked.
    NoVerification,
}

impl Gap {
    /// Stable name used in the session state and the JSON output.
    pub fn key(self) -> &'static str {
        match self {
            Self::NoTarget => "no_target",
            Self::BroadScope => "broad_scope",
            Self::NoSuccessCriteria => "no_success_criteria",
            Self::NoVerification => "no_verification",
        }
    }

    /// What is missing, as a phrase for the agent.
    pub fn describe(self) -> &'static str {
        match self {
            Self::NoTarget => "no file, function, or doc it points at",
            Self::BroadScope => "a very broad scope",
            Self::NoSuccessCriteria => "no statement of what the result should be",
            Self::NoVerification => "no hint of how to verify it",
        }
    }

    /// The question that closes the gap.
    pub fn question(self) -> &'static str {
        match self {
            Self::NoTarget => "Which file, function, screen, or endpoint should change?",
            Self::BroadScope => {
                "Can this start with one module or one behaviour instead of everything?"
            }
            Self::NoSuccessCriteria => {
                "What should be true when this is done (expected result, error cases)?"
            }
            Self::NoVerification => {
                "How should it be verified (a test, an API scenario, a manual check)?"
            }
        }
    }

    /// One-line advice for the user, shown at most once per session.
    pub fn tip(self) -> &'static str {
        match self {
            Self::NoTarget => {
                "name the file, function, or screen you mean so the work starts in the right place."
            }
            Self::BroadScope => "pick one small piece first; \"everything\" is hard to check.",
            Self::NoSuccessCriteria => {
                "describe what \"done\" looks like, for example \"returns 404 when the order does not exist\"."
            }
            Self::NoVerification => "say how to check it: a test to pass or a page to open.",
        }
    }
}

/// Finds the gaps of `text`, most important first.
pub fn detect(text: &str, gathered: &Gathered) -> Vec<Gap> {
    let padded = padded_words(text);
    let mut gaps = Vec::new();
    if !gathered.has_target() {
        gaps.push(Gap::NoTarget);
    }
    if mentions_any(&padded, BROAD_PHRASES) {
        gaps.push(Gap::BroadScope);
    }
    if !mentions_any(&padded, SUCCESS_PHRASES) && !has_status_code(&padded) {
        gaps.push(Gap::NoSuccessCriteria);
    }
    if !mentions_any(&padded, VERIFICATION_PHRASES) {
        gaps.push(Gap::NoVerification);
    }
    gaps
}

/// `true` when the gaps are worth interrupting for. A missing verification hint alone is not:
/// it is true of most prompts and the agent can pick the repo's own checks.
pub fn worth_coaching(gaps: &[Gap]) -> bool {
    gaps.iter().any(|g| *g != Gap::NoVerification)
}

/// Lowercase, accent-folded words joined by single spaces and padded, so `contains(" word ")`
/// matches whole words and phrases only.
fn padded_words(text: &str) -> String {
    let folded = fold_accents(text);
    let words: Vec<&str> =
        folded.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    format!(" {} ", words.join(" "))
}

fn mentions_any(padded: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| padded.contains(&format!(" {p} ")))
}

/// HTTP status codes written as a bare number (`404`, `201`).
fn has_status_code(padded: &str) -> bool {
    padded
        .split(' ')
        .any(|w| w.len() == 3 && w.parse::<u16>().is_ok_and(|n| (100..600).contains(&n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::prompt::refs::CodeRef;

    fn with_target() -> Gathered {
        let code = CodeRef {
            name: "Order".into(),
            kind: "class".into(),
            file: "o.ts".into(),
            line: Some(1),
        };
        Gathered { code: vec![code], ..Gathered::default() }
    }

    fn gaps(text: &str, gathered: &Gathered) -> Vec<Gap> {
        detect(text, gathered)
    }

    #[test]
    fn a_vague_english_request_lacks_everything() {
        let got = gaps("fix everything about the login", &Gathered::default());
        assert_eq!(
            got,
            [Gap::NoTarget, Gap::BroadScope, Gap::NoSuccessCriteria, Gap::NoVerification]
        );
    }

    #[test]
    fn a_vague_spanish_request_lacks_the_same_things() {
        let got = gaps("arregla todo lo del login", &Gathered::default());
        assert_eq!(
            got,
            [Gap::NoTarget, Gap::BroadScope, Gap::NoSuccessCriteria, Gap::NoVerification]
        );
        assert!(gaps("arregla toda la app", &Gathered::default()).contains(&Gap::BroadScope));
    }

    #[test]
    fn success_phrases_in_both_languages_close_the_gap() {
        for text in [
            "the endpoint should reject empty carts",
            "it must return the total",
            "returns 404 when missing",
            "el carrito debe sumar el IVA",
            "tiene que mostrar el total",
            "quiero que funcione sin errores",
            "devuelve el total",
            "respond with 409",
        ] {
            assert!(!gaps(text, &with_target()).contains(&Gap::NoSuccessCriteria), "{text}");
        }
        assert!(
            gaps("add a discount to the order", &with_target()).contains(&Gap::NoSuccessCriteria)
        );
    }

    #[test]
    fn verification_hints_in_both_languages_close_the_gap() {
        for text in [
            "add it and run the tests",
            "agrega el campo y escribe una prueba",
            "then verify with qa",
        ] {
            assert!(!gaps(text, &with_target()).contains(&Gap::NoVerification), "{text}");
        }
        assert!(gaps("add a discount", &with_target()).contains(&Gap::NoVerification));
    }

    #[test]
    fn a_resolved_target_or_a_named_file_closes_the_target_gap() {
        assert!(!gaps("whatever", &with_target()).contains(&Gap::NoTarget));
        let file = Gathered { names_a_file: true, ..Gathered::default() };
        assert!(!gaps("change order.ts", &file).contains(&Gap::NoTarget));
    }

    #[test]
    fn matching_is_by_whole_word_not_substring() {
        // "shoulder" contains "should" and "contest" contains "test".
        let got = gaps("tune the shoulder contest banner", &with_target());
        assert!(got.contains(&Gap::NoSuccessCriteria));
        assert!(got.contains(&Gap::NoVerification));
    }

    #[test]
    fn bare_all_is_not_a_broad_scope() {
        assert!(
            !gaps("list all orders with a discount", &with_target()).contains(&Gap::BroadScope)
        );
    }

    #[test]
    fn a_missing_verification_hint_alone_is_not_worth_coaching() {
        assert!(!worth_coaching(&[Gap::NoVerification]));
        assert!(!worth_coaching(&[]));
        assert!(worth_coaching(&[Gap::NoSuccessCriteria, Gap::NoVerification]));
    }
}

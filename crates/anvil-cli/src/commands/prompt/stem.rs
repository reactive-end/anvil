//! A small, deterministic stemmer for English and Spanish, so "cancelled" in a prompt meets
//! `CancelOrderUseCase` in the code. It is applied to both sides of every word comparison, so what
//! matters is that related forms agree, not that the stem is a real word (`crear` gives `cre`).
//!
//! Rules, in order, on the lowercase, accent-folded word (the first one that applies wins):
//! 1. Words shorter than 4 letters, or with anything but ASCII letters, are left alone, and no rule
//!    leaves fewer than 3 letters.
//! 2. Plural: `-ies` becomes `y`; `-es` is dropped after `ss`, `x`, `z`, `ch`, `sh` or `n`; any other
//!    `-s` is dropped, except after `ss`, `us` or `is` (`address`, `status`, `analysis`).
//! 3. Derivation: Spanish `-acion`, `-ado`, `-ada`, `-ido`, `-ida` are dropped; English `-ied`
//!    becomes `y`, `-ing` and `-ed` are dropped; the Spanish infinitives `-ar` and `-ir` are dropped.
//!    Infinitive `-er` is not: it would turn `order` and `customer` into `ord` and `custom`.
//! 4. A final silent `e` is dropped (`create` and `creating` agree, `papeles` and `papel` agree).
//! 5. A final doubled consonant is undoubled (`cancelled` -> `cancell` -> `cancel`).

use std::collections::HashSet;

use super::stopwords::fold_accents;

/// Words shorter than this are never stemmed.
const MIN_INPUT_CHARS: usize = 4;
/// No rule leaves a stem shorter than this.
const MIN_STEM_CHARS: usize = 3;
/// Endings after which a plural is `-es` (`classes`, `boxes`, `lines`, `acciones`).
const ES_AFTER: &[&str] = &["ss", "x", "z", "ch", "sh", "n"];
/// Endings whose `s` is part of the word, not a plural.
const KEEP_S_AFTER: &[&str] = &["ss", "us", "is"];
/// Derivational suffixes with what replaces them, longest first within the same family.
const DERIVATIONS: &[(&str, &str)] = &[
    ("acion", ""),
    ("ado", ""),
    ("ada", ""),
    ("ido", ""),
    ("ida", ""),
    ("ied", "y"),
    ("ing", ""),
    ("ed", ""),
    ("ar", ""),
    ("ir", ""),
];

/// The stem of `word`: lowercase, accent-folded, and reduced as described in the module docs.
pub fn stem(word: &str) -> String {
    let word = fold_accents(word);
    if word.len() < MIN_INPUT_CHARS || !word.chars().all(|c| c.is_ascii_lowercase()) {
        return word;
    }
    let word = strip_plural(&word).unwrap_or(word);
    let word = strip_derivation(&word).unwrap_or(word);
    let word = drop_silent_e(word);
    undouble(word)
}

/// `words` as a set of stems, so "order" and "orders" count once.
pub fn stem_set<'a>(words: impl IntoIterator<Item = &'a String>) -> HashSet<String> {
    words.into_iter().map(|w| stem(w)).collect()
}

/// The words followed by the stems that differ from them: what a lookup that matches whole words
/// (the graph, the docs, the memory index) is given, so it finds `cancel` for "cancelled".
pub fn with_stems(words: &[String]) -> String {
    let mut terms: Vec<String> = words.to_vec();
    for word in words {
        let stemmed = stem(word);
        if !terms.contains(&stemmed) {
            terms.push(stemmed);
        }
    }
    terms.join(" ")
}

fn strip_plural(word: &str) -> Option<String> {
    if let Some(root) = word.strip_suffix("ies") {
        return long_enough(format!("{root}y"));
    }
    if let Some(root) = word.strip_suffix("es")
        && ES_AFTER.iter().any(|end| root.ends_with(end))
    {
        return long_enough(root.to_owned());
    }
    let root = word.strip_suffix('s')?;
    if KEEP_S_AFTER.iter().any(|end| word.ends_with(end)) {
        return None;
    }
    long_enough(root.to_owned())
}

fn strip_derivation(word: &str) -> Option<String> {
    DERIVATIONS.iter().find_map(|(suffix, replacement)| {
        let root = word.strip_suffix(suffix)?;
        long_enough(format!("{root}{replacement}"))
    })
}

fn drop_silent_e(word: String) -> String {
    match word.strip_suffix('e') {
        Some(root) if word.len() >= MIN_INPUT_CHARS && root.len() >= MIN_STEM_CHARS => {
            root.to_owned()
        }
        _ => word,
    }
}

fn undouble(mut word: String) -> String {
    let mut last = word.chars().rev();
    let doubled = match (last.next(), last.next()) {
        (Some(a), Some(b)) => a == b && !"aeiou".contains(a),
        _ => false,
    };
    if doubled && word.len() > MIN_STEM_CHARS {
        word.pop();
    }
    word
}

fn long_enough(stem: String) -> Option<String> {
    (stem.len() >= MIN_STEM_CHARS).then_some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_inflections_of_one_verb_agree() {
        for form in ["cancelled", "cancelling", "cancels", "cancel"] {
            assert_eq!(stem(form), "cancel", "{form}");
        }
        assert_eq!(stem("stopped"), "stop");
        assert_eq!(stem("stopping"), stem("stop"));
        assert_eq!(stem("orders"), stem("order"));
        assert_eq!(stem("order"), "order");
    }

    #[test]
    fn a_silent_e_does_not_split_the_forms() {
        let forms = ["create", "created", "creates", "creating"];
        assert!(forms.iter().all(|f| stem(f) == "creat"), "{:?}", forms.map(stem));
        assert_eq!(stem("releases"), stem("released"));
        assert_eq!(stem("nodes"), stem("node"));
    }

    #[test]
    fn plurals_and_endings_of_names_are_handled() {
        assert_eq!(stem("categories"), stem("category"));
        assert_eq!(stem("copied"), stem("copy"));
        assert_eq!(stem("classes"), stem("class"));
        assert_eq!(stem("addresses"), stem("address"));
        assert_eq!(stem("status"), "status");
        assert_eq!(stem("statuses"), stem("status"));
        assert_eq!(stem("stock"), "stock");
    }

    #[test]
    fn spanish_forms_of_one_verb_agree() {
        for form in ["cancelado", "cancelada", "cancelados", "cancelar", "cancelación"] {
            assert_eq!(stem(form), "cancel", "{form}");
        }
        assert_eq!(stem("cancelacion"), stem("cancelación"));
        assert_eq!(stem("cancelaciones"), "cancel");
        assert_eq!(stem("pedidos"), stem("pedido"));
        assert_eq!(stem("crear"), "cre");
        assert_eq!(stem("creación"), "cre");
        assert_eq!(stem("papeles"), stem("papel"));
    }

    #[test]
    fn short_words_and_non_words_are_left_alone() {
        for word in ["use", "was", "ing", "red", "ado", "10", "v2s", "v2es"] {
            assert_eq!(stem(word), word, "{word}");
        }
        assert_eq!(stem("Años"), "ano");
        assert_eq!(stem("string"), "str");
        assert_eq!(stem("used"), "used");
        assert!(stem("buses").len() >= MIN_STEM_CHARS);
    }

    #[test]
    fn with_stems_adds_only_the_stems_that_differ() {
        let words = vec!["cancelled".to_owned(), "stock".to_owned(), "orders".to_owned()];
        assert_eq!(with_stems(&words), "cancelled stock orders cancel order");
    }
}

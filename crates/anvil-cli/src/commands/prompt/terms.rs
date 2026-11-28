//! Turns the text of a prompt into the terms worth looking up: identifiers first (they name code
//! exactly), then the content words left once English and Spanish filler is gone.

use super::stopwords::{fold_accents, is_stopword};

/// Identifiers looked up in the graph per prompt; more would cost latency for little gain.
pub const MAX_IDENTS: usize = 6;
/// Content words kept per prompt.
const MAX_WORDS: usize = 10;
/// Quoted or backticked text longer than this is prose, not a name.
const MAX_QUOTED_CHARS: usize = 80;
/// File extensions that make a bare word such as `order.ts` a file name.
const FILE_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "rs", "py", "md", "json", "kt", "dart", "php", "java", "go",
    "vue", "css", "html", "yml", "yaml", "toml", "sql", "gd", "svelte", "cs", "rb", "swift",
];

/// What an identifier names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentKind {
    /// A function, class, constant, or other symbol (`searchRepo`, `Order`, `order-item`).
    Symbol,
    /// A file or folder path (`src/x.ts`, `order.ts`).
    Path,
}

/// A code-looking token of the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub text: String,
    pub kind: IdentKind,
}

/// Everything extracted from one prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terms {
    /// At most [`MAX_IDENTS`], in order of appearance.
    pub idents: Vec<Ident>,
    /// Lowercase content words that are not part of an identifier, filler removed.
    pub words: Vec<String>,
    /// Lowercase pieces of the identifiers (`search_repo` gives `search`, `repo`).
    pub ident_words: Vec<String>,
}

impl Terms {
    /// `true` when the prompt gave nothing to look up.
    pub fn is_empty(&self) -> bool {
        self.idents.is_empty() && self.words.is_empty()
    }

    /// Plain words followed by identifier pieces: what docs and memory are searched with. Graph
    /// symbols are looked up by `words` alone, because the identifiers were resolved exactly.
    pub fn all_words(&self) -> Vec<String> {
        let mut all = self.words.clone();
        all.extend(self.ident_words.iter().filter(|w| !self.words.contains(w)).cloned());
        all
    }
}

/// Why a prompt gets no assistance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// A slash command such as `/compact`.
    SlashCommand,
    /// Text injected by the harness (`<task-notification>`), not typed by the user.
    Injected,
    /// Fewer words than `min_words`: a follow-up like "yes, go ahead".
    TooShort,
}

/// Decides whether `text` is a prompt worth assisting.
pub fn skip_reason(text: &str, min_words: usize) -> Option<Skip> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('/') {
        Some(Skip::SlashCommand)
    } else if trimmed.starts_with('<') {
        Some(Skip::Injected)
    } else if text.split_whitespace().count() < min_words {
        Some(Skip::TooShort)
    } else {
        None
    }
}

/// Extracts identifiers and content words from `text`.
pub fn extract(text: &str) -> Terms {
    let (quoted, rest) = split_quoted(text);
    let mut idents: Vec<Ident> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut ident_words: Vec<String> = Vec::new();
    for token in &quoted {
        let kind = classify(token, false).unwrap_or(IdentKind::Symbol);
        push_ident(&mut idents, token, kind);
        push_words(&mut ident_words, token);
    }
    let mut sentence_start = true;
    for raw in rest.split_whitespace() {
        let token = raw.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
        if !token.is_empty() {
            match classify(token, sentence_start) {
                Some(kind) => {
                    push_ident(&mut idents, token, kind);
                    push_words(&mut ident_words, token);
                }
                None => push_words(&mut words, token),
            }
        }
        sentence_start = raw.ends_with(['.', '!', '?', ':']);
    }
    idents.truncate(MAX_IDENTS);
    words.truncate(MAX_WORDS);
    ident_words.truncate(MAX_WORDS);
    Terms { idents, words, ident_words }
}

/// Pulls backticked and double-quoted segments out of `text`. Single-word segments are returned
/// to be treated as names; the rest of the text keeps the multi-word ones as plain prose.
fn split_quoted(text: &str) -> (Vec<String>, String) {
    let mut quoted = Vec::new();
    let mut rest = String::with_capacity(text.len());
    let mut open: Option<(char, String)> = None;
    for ch in text.chars() {
        match (&mut open, ch) {
            (None, '`' | '"') => open = Some((ch, String::new())),
            (Some((quote, _)), c) if c == *quote => {
                if let Some((_, segment)) = open.take() {
                    take_segment(segment, &mut quoted, &mut rest);
                }
            }
            (Some((_, segment)), c) => segment.push(c),
            (None, c) => rest.push(c),
        }
    }
    if let Some((_, unclosed)) = open {
        rest.push(' ');
        rest.push_str(&unclosed);
    }
    (quoted, rest)
}

fn take_segment(segment: String, quoted: &mut Vec<String>, rest: &mut String) {
    let trimmed = segment.trim();
    let one_word = !trimmed.is_empty() && !trimmed.contains(char::is_whitespace);
    if one_word && trimmed.chars().count() <= MAX_QUOTED_CHARS {
        quoted.push(trimmed.to_owned());
    } else {
        rest.push(' ');
        rest.push_str(trimmed);
        rest.push(' ');
    }
}

fn push_ident(idents: &mut Vec<Ident>, text: &str, kind: IdentKind) {
    if !idents.iter().any(|i| i.text == text) {
        idents.push(Ident { text: text.to_owned(), kind });
    }
}

/// Adds the lowercase pieces of `token` (split on anything that is not a letter or digit, and on
/// camelCase boundaries) that are neither filler nor numbers.
fn push_words(words: &mut Vec<String>, token: &str) {
    for piece in split_pieces(token) {
        let word = fold_accents(&piece);
        let is_number = word.chars().all(|c| c.is_ascii_digit());
        if word.chars().count() >= 3 && !is_number && !is_stopword(&word) && !words.contains(&word)
        {
            words.push(word);
        }
    }
}

/// Lowercase, accent-folded pieces of `text`, split on non-alphanumerics and camelCase humps, so a
/// symbol such as `calcularDescuento` can be compared with the words of a prompt.
pub fn tokens(text: &str) -> Vec<String> {
    split_pieces(text).iter().map(|p| fold_accents(p)).collect()
}

fn split_pieces(token: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in token.chars() {
        if !ch.is_alphanumeric() {
            flush(&mut pieces, &mut current);
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower {
            flush(&mut pieces, &mut current);
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        current.push(ch);
    }
    flush(&mut pieces, &mut current);
    pieces
}

fn flush(pieces: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        pieces.push(std::mem::take(current));
    }
}

/// What kind of name `token` is, or `None` for an ordinary word.
fn classify(token: &str, sentence_start: bool) -> Option<IdentKind> {
    if token.chars().count() < 2 || !token.chars().any(char::is_alphabetic) {
        return None;
    }
    if is_path(token) {
        return Some(IdentKind::Path);
    }
    let symbol = has_inner_separator(token) || is_mixed_case(token) || token.contains('_');
    let capitalized = !sentence_start && is_capitalized_word(token);
    (symbol || capitalized).then_some(IdentKind::Symbol)
}

fn is_path(token: &str) -> bool {
    has_file_extension(token) || has_separator_path(token)
}

/// `true` for a token ending in a known file extension, such as `order.service.ts`.
pub fn has_file_extension(token: &str) -> bool {
    token.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    })
}

/// A `/` or `\` path with at least one real word in it, so "and/or" is not one.
fn has_separator_path(token: &str) -> bool {
    const SEPARATORS: [char; 2] = ['/', '\\'];
    token.contains(SEPARATORS)
        && token.split(SEPARATORS).any(|seg| seg.chars().count() >= 3 && !is_stopword(seg))
}

/// `kebab-case` or `dotted.name`: only name characters, at least two segments, each segment at
/// least two characters long (so `e.g`, `v1.2` and `3.14` are not names).
fn has_inner_separator(token: &str) -> bool {
    let only_name_chars =
        token.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_'));
    if !only_name_chars || !token.contains(['-', '.']) {
        return false;
    }
    let segments: Vec<&str> = token.split(['-', '.']).collect();
    segments.iter().all(|s| s.chars().count() >= 2 && s.chars().any(char::is_alphabetic))
}

/// `camelCase` (a lowercase start with a later hump) or `PascalCase` with at least two humps.
fn is_mixed_case(token: &str) -> bool {
    let mut humps = 0;
    let mut prev_lower = false;
    for ch in token.chars() {
        if ch.is_uppercase() && (prev_lower || humps == 0) {
            humps += 1;
        }
        prev_lower = ch.is_lowercase();
    }
    let starts_lower = token.chars().next().is_some_and(char::is_lowercase);
    token.chars().any(char::is_lowercase) && (humps >= 2 || (humps == 1 && starts_lower))
}

/// A capitalised word in the middle of a sentence, such as `Order` in "add a rule to Order".
fn is_capitalized_word(token: &str) -> bool {
    let mut chars = token.chars();
    let first_upper = chars.next().is_some_and(char::is_uppercase);
    first_upper
        && token.chars().count() >= 3
        && chars.all(char::is_alphanumeric)
        && token.chars().any(char::is_lowercase)
        && !is_stopword(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idents(text: &str) -> Vec<(String, IdentKind)> {
        extract(text).idents.into_iter().map(|i| (i.text, i.kind)).collect()
    }

    fn sym(name: &str) -> (String, IdentKind) {
        (name.to_owned(), IdentKind::Symbol)
    }

    #[test]
    fn finds_every_identifier_style() {
        let text =
            "rename searchRepo and OrderItem and search_repo and order-item and config.qa.api";
        assert_eq!(
            idents(text),
            vec![
                sym("searchRepo"),
                sym("OrderItem"),
                sym("search_repo"),
                sym("order-item"),
                sym("config.qa.api"),
            ]
        );
    }

    #[test]
    fn finds_paths_and_file_names() {
        let got = idents("look at src/orders/order.ts and also order.service.ts or crates\\core");
        assert_eq!(
            got,
            vec![
                ("src/orders/order.ts".to_owned(), IdentKind::Path),
                ("order.service.ts".to_owned(), IdentKind::Path),
                ("crates\\core".to_owned(), IdentKind::Path),
            ]
        );
    }

    #[test]
    fn backticked_and_quoted_single_words_are_names_but_prose_is_not() {
        let got = idents("the `login` handler and the \"Cart\" page and \"a long phrase here\"");
        assert_eq!(got, vec![sym("login"), sym("Cart")]);
        assert!(extract("say \"a long phrase here\" today").words.contains(&"phrase".to_owned()));
    }

    #[test]
    fn a_capitalised_word_is_a_name_only_inside_a_sentence() {
        assert!(idents("Make the checkout faster").is_empty());
        assert_eq!(idents("add a rule to Order today"), vec![sym("Order")]);
        assert!(idents("It broke. Please look").is_empty());
    }

    #[test]
    fn ordinary_words_and_numbers_are_not_identifiers() {
        assert!(idents("add a discount when quantity is over 10 or 3.14 or e.g. soon").is_empty());
    }

    #[test]
    fn english_and_spanish_filler_is_removed_from_the_words() {
        let en = extract("please make the checkout page show all the discounts").words;
        assert_eq!(en, vec!["checkout", "page", "discounts"]);
        let es = extract("necesito que arregles el descuento del carrito por favor").words;
        assert_eq!(es, vec!["arregles", "descuento", "carrito"]);
    }

    #[test]
    fn identifier_pieces_are_kept_apart_from_plain_words() {
        let terms = extract("make searchRepo rank stale_concepts lower");
        assert_eq!(terms.words, vec!["rank", "lower"]);
        assert_eq!(terms.ident_words, vec!["search", "repo", "stale", "concepts"]);
        assert_eq!(terms.all_words(), vec!["rank", "lower", "search", "repo", "stale", "concepts"]);
    }

    #[test]
    fn caps_the_lookups() {
        let text = "a_b1 c_d2 e_f3 g_h4 i_j5 k_l6 m_n7 o_p8 q_r9";
        assert_eq!(extract(text).idents.len(), MAX_IDENTS);
    }

    #[test]
    fn skips_slash_commands_injected_text_and_short_prompts() {
        assert_eq!(skip_reason("/compact", 4), Some(Skip::SlashCommand));
        assert_eq!(skip_reason("  /model opus please now", 4), Some(Skip::SlashCommand));
        assert_eq!(
            skip_reason("<task-notification>done with the whole thing</task-notification>", 4),
            Some(Skip::Injected)
        );
        assert_eq!(skip_reason("yes do it", 4), Some(Skip::TooShort));
        assert_eq!(skip_reason("si, hazlo ya", 4), Some(Skip::TooShort));
        assert_eq!(skip_reason("make search_repo rank stale concepts lower", 4), None);
    }
}

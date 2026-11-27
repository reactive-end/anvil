//! Decides whether a query is clearly literal text that symbol search cannot match.

/// Punctuation a symbol or concept name never contains, so a query with it is text to grep for.
const LITERAL_PUNCTUATION: &[char] = &[
    '(', ')', '[', ']', '{', '}', '<', '>', '=', '!', '?', '*', '+', '|', '\\', '^', '$', ';', ',',
    '"', '\'', '`', '@', '#', '%', '&', '~',
];

/// A query split into its searchable text and what kind of text it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    /// The query without surrounding quotes.
    pub text: String,
    /// The whole query was wrapped in quotes.
    pub quoted: bool,
}

impl Literal {
    /// Trims the query and strips one pair of matching surrounding quotes or backticks.
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        let mut chars = trimmed.chars();
        let (first, last) = (chars.next(), chars.next_back());
        let wrapped = match (first, last) {
            (Some(a), Some(b)) => a == b && matches!(a, '"' | '\'' | '`') && trimmed.len() > 2,
            _ => false,
        };
        if wrapped {
            let inner = &trimmed[1..trimmed.len() - 1];
            return Self { text: inner.trim().to_owned(), quoted: true };
        }
        Self { text: trimmed.to_owned(), quoted: false }
    }

    /// Quoted, or containing punctuation that identifiers and file names never have.
    pub fn is_literal(&self) -> bool {
        self.quoted || self.text.contains(LITERAL_PUNCTUATION)
    }

    /// The text reads as an intended regular expression rather than a literal string.
    pub fn looks_regex(&self) -> bool {
        if self.quoted {
            return false;
        }
        let text = &self.text;
        text.contains(['\\', '^', '$', '|'])
            || text.contains(".*")
            || text.contains(".+")
            || (text.contains('[') && text.contains(']'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_stripped_and_make_the_query_literal() {
        let q = Literal::parse("  \"hello world\"  ");
        assert_eq!(q.text, "hello world");
        assert!(q.quoted && q.is_literal() && !q.looks_regex());
    }

    #[test]
    fn identifiers_and_paths_are_not_literal() {
        for plain in ["check_security_guard", "fn check_security_guard", "src/lib.rs", "a::b.c"] {
            assert!(!Literal::parse(plain).is_literal(), "{plain}");
        }
    }

    #[test]
    fn punctuation_makes_it_literal_and_only_regex_syntax_makes_it_a_regex() {
        assert!(Literal::parse("foo(").is_literal());
        assert!(!Literal::parse("foo(").looks_regex());
        assert!(Literal::parse(r"fn\s+foo").looks_regex());
        assert!(Literal::parse("a.*b").looks_regex());
    }
}

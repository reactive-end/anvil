//! Words that say nothing about where in the code a request lives. People write to the agent in
//! English and in Spanish, so both lists apply to every prompt.

/// Articles, pronouns, prepositions, and the verbs that open a request ("add", "fix", "make").
/// Plain ASCII: the word is accent-folded before the lookup.
const STOPWORDS: &[&str] = &[
    // English
    "about", "add", "all", "also", "and", "any", "are", "because", "been", "being", "but", "can",
    "change", "could", "create", "did", "does", "doing", "done", "each", "else", "fix", "for",
    "from", "get", "give", "has", "have", "how", "into", "its", "just", "let", "like", "make",
    "more", "need", "not", "now", "our", "out", "over", "please", "should", "show", "some", "than",
    "that", "the", "their", "them", "then", "there", "these", "they", "this", "those", "update",
    "use", "very", "want", "was", "were", "what", "when", "where", "which", "while", "who", "why",
    "will", "with", "would", "you", "your", // Spanish
    "agrega", "agregar", "algo", "arregla", "arreglar", "cada", "cambia", "cambiar", "como", "con",
    "crea", "crear", "cual", "cuando", "dame", "del", "desde", "donde", "ese", "esa", "eso",
    "esta", "este", "esto", "favor", "haz", "hacer", "hay", "las", "los", "mas", "muy", "necesito",
    "otro", "otra", "para", "pero", "por", "porque", "puedes", "quiero", "que", "sea", "ser",
    "sin", "sobre", "son", "tambien", "todo", "una", "uno", "unos", "unas", "usa", "usar", "ver",
    "ya",
];

/// `true` for a word that is only filler. Case and Spanish accents are folded, so "Más" matches.
pub fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&fold_accents(word).as_str())
}

/// Lowercases `text` and replaces the accented Spanish vowels and `ñ` with their plain letters.
pub fn fold_accents(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filler_is_dropped_in_both_languages() {
        for word in ["the", "Please", "make", "para", "Más", "AGREGA", "también"] {
            assert!(is_stopword(word), "{word}");
        }
        for word in ["order", "login", "descuento", "repo"] {
            assert!(!is_stopword(word), "{word}");
        }
    }

    #[test]
    fn accents_fold_to_plain_letters() {
        assert_eq!(fold_accents("Año Código Pingüino"), "ano codigo pinguino");
    }
}

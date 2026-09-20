//! Stop-word filtering for FTS queries.

use std::collections::HashSet;
use std::sync::LazyLock;

static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a",
        "an",
        "the",
        "this",
        "that",
        "these",
        "those",
        "i",
        "me",
        "my",
        "we",
        "our",
        "you",
        "your",
        "he",
        "she",
        "it",
        "they",
        "him",
        "her",
        "its",
        "them",
        "us",
        "is",
        "are",
        "was",
        "were",
        "be",
        "been",
        "being",
        "have",
        "has",
        "had",
        "do",
        "does",
        "did",
        "will",
        "would",
        "could",
        "should",
        "can",
        "may",
        "might",
        "in",
        "on",
        "at",
        "to",
        "for",
        "of",
        "with",
        "by",
        "from",
        "about",
        "into",
        "through",
        "during",
        "before",
        "after",
        "above",
        "below",
        "and",
        "or",
        "but",
        "if",
        "then",
        "because",
        "as",
        "while",
        "when",
        "where",
        "what",
        "which",
        "who",
        "how",
        "why",
        "thing",
        "things",
        "stuff",
        "something",
        "anything",
        "everything",
        "nothing",
        "please",
        "just",
        "also",
        "very",
        "really",
        "quite",
        "rather",
        "so",
        "too",
        "than",
        "then",
        "there",
        "here",
        "some",
        "any",
        "all",
        "each",
        "every",
        "both",
        "few",
        "more",
        "most",
        "other",
        "such",
        "no",
        "nor",
        "not",
        "only",
        "own",
        "same",
        "than",
        "too",
        "very",
        "remember",
        "memory",
        "recall",
    ]
    .into_iter()
    .collect()
});

/// Lowercase, split on non-alphanumeric, drop stop words, dedup preserving order.
pub fn extract_keywords(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in query.split(|c: char| !c.is_alphanumeric()) {
        let w = raw.to_ascii_lowercase();
        if w.len() < 2 || STOP_WORDS.contains(w.as_str()) {
            continue;
        }
        if seen.insert(w.clone()) {
            out.push(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_stop_words() {
        let k = extract_keywords("remember the rust ownership thing");
        assert_eq!(k, vec!["rust".to_string(), "ownership".to_string()]);
    }
}

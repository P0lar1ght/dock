//! FTS5 text normalization shared by indexing and query construction.
//!
//! SQLite's built-in FTS5 tokenizers (`unicode61`, `porter`) do not segment CJK
//! text: a run like `语言偏好` is stored as one long token, so `MATCH '底栏'`
//! cannot hit a line that contains `底栏` inside a longer run. The query side
//! already splits on non-alphanumerics, which keeps the run intact for Chinese,
//! so a multi-word Chinese query degrades to one giant token that matches
//! nothing.
//!
//! This module normalizes both the stored text and the query the same way:
//! every maximal run of CJK ideographs is expanded into overlapping bigrams
//! (`语言偏好` → `语言 言偏 偏好`), which the default `unicode61` tokenizer can
//! then index and match as ordinary words. Non-CJK text is left alone, so
//! English stopwords, code identifiers, and punctuation behave as before.
//!
//! Escaping: FTS5 treats `.` `+` `-` `*` `"` and friends as syntax. A query term
//! containing them (e.g. `dock.1`, `agent/pre-step`) is wrapped as an FTS5
//! phrase (`"dock.1"`) so it matches the stored token verbatim instead of
//! raising a syntax error.

/// Quote an FTS5 MATCH term as a phrase so characters like `.` `/` `+` `-` `*`
/// lose their syntactic meaning. Embedded double quotes are doubled.
pub fn escape_fts_term(term: &str) -> String {
    // A bare alphanumeric term (optionally with `_`) needs no quoting.
    let bare = term.chars().all(|c| c.is_alphanumeric() || c == '_');
    if bare {
        return term.to_string();
    }
    let mut out = String::with_capacity(term.len() + 2);
    out.push('"');
    for c in term.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// `true` for CJK ideographs and related scripts that SQLite does not segment.
///
/// Covers the blocks that actually appear in mixed technical notes: CJK
/// Unified Ideographs (incl. Extension A and the rare ranges fed by `U+2EB0`
/// Kangxi), Hiragana, Katakana, Hangul, and CJK punctuation and symbols.
pub fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{3400}'..='\u{4DBF}'   // CJK Unified Ideographs Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
    )
}

/// Segment a CJK run into overlapping bigrams.
///
/// A run of length 1 yields that single character; longer runs yield every
/// adjacent pair. `语言偏好` → `["语言", "言偏", "偏好"]`.
pub fn cjk_bigrams(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    if chars.len() == 1 {
        return vec![chars.iter().collect()];
    }
    (0..chars.len() - 1)
        .map(|i| chars.get(i..i + 2).unwrap_or(&[]).iter().collect())
        .collect()
}

/// Split text into segments of CJK and non-CJK characters.
fn split_cjk_segments(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let first = rest.chars().next().unwrap();
        let cjk = is_cjk(first);
        let boundary = rest
            .char_indices()
            .skip(1)
            .find(|(_, c)| is_cjk(*c) != cjk)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        out.push((cjk, &rest[..boundary]));
        rest = &rest[boundary..];
    }
    out
}

/// Expand CJK runs into bigrams and keep the rest verbatim.
///
/// Returns the tokens in order. Whitespace is discarded, since callers join
/// them with a separator. For a non-CJK segment the tokens are the original
/// words, preserving internal characters like `-` and `.` in `dock.1`.
pub fn tokenize_for_fts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (cjk, segment) in split_cjk_segments(text) {
        if cjk {
            for token in cjk_bigrams(segment) {
                if !token.trim().is_empty() {
                    out.push(token);
                }
            }
            continue;
        }
        for token in segment.split_whitespace() {
            out.push(token.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_run_becomes_bigrams() {
        assert_eq!(tokenize_for_fts("语言偏好"), vec!["语言", "言偏", "偏好"]);
    }

    #[test]
    fn mixed_line_keeps_ascii_tokens() {
        let got = tokenize_for_fts("协议 dock.1 与 embed sdk");
        assert!(got.contains(&"dock.1".to_string()));
        assert!(got.contains(&"embed".to_string()));
        assert!(got.contains(&"sdk".to_string()));
        assert!(got.iter().any(|t| t.contains('协')));
    }

    #[test]
    fn adjacent_cjk_words_still_split_into_bigrams() {
        // “语言偏好” and “底栏” written without spaces still tokenize as a
        // continuous run; the bigrams bridge the two words.
        let got = tokenize_for_fts("语言偏好底栏");
        assert!(got.contains(&"偏好".to_string()));
        assert!(got.contains(&"底栏".to_string()));
    }

    #[test]
    fn pure_ascii_unchanged_by_tokenizer() {
        assert_eq!(
            tokenize_for_fts("cargo test -p cordis-spine"),
            vec!["cargo", "test", "-p", "cordis-spine"]
        );
    }

    #[test]
    fn single_cjk_char_yields_itself() {
        assert_eq!(tokenize_for_fts("语"), vec!["语"]);
    }

    #[test]
    fn empty_and_whitespace_only() {
        assert!(tokenize_for_fts("").is_empty());
        assert!(tokenize_for_fts("   \n\t ").is_empty());
    }

    #[test]
    fn escapes_dotted_and_slashed_terms() {
        assert_eq!(escape_fts_term("dock.1"), "\"dock.1\"");
        assert_eq!(escape_fts_term("agent/pre-step"), "\"agent/pre-step\"");
        assert_eq!(escape_fts_term("rust"), "rust");
    }

    #[test]
    fn escapes_terms_with_fts_syntax_chars() {
        assert_eq!(escape_fts_term("a+b"), "\"a+b\"");
        assert_eq!(escape_fts_term("a*b"), "\"a*b\"");
    }

    #[test]
    fn doubles_embedded_quotes() {
        assert_eq!(escape_fts_term("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}

//! @-context detection copied from grok pager `views/file_search/context.rs`.
//! Drill-prefix (whitespace inside a directory name) is omitted.

use std::ops::Range;

/// Context for the current @-completion token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtContext {
    /// Byte range including the leading `@`.
    pub range: Range<usize>,
    pub cursor: usize,
    /// Text after `@` up to the cursor.
    pub query: String,
}

impl AtContext {
    pub fn is_dir_mode(&self) -> bool {
        self.query.ends_with('/')
    }

    pub fn is_hidden_mode(&self) -> bool {
        self.query.starts_with('!')
    }

    pub fn matcher_query(&self) -> &str {
        self.query.strip_prefix('!').unwrap_or(&self.query)
    }

    /// Range to replace when inserting a path (keeps `@` and optional `!`).
    pub fn path_range(&self) -> Range<usize> {
        let prefix = 1 + usize::from(self.is_hidden_mode());
        self.range.start + prefix..self.range.end
    }
}

/// Detect an @-token around `cursor`. `None` if the `@` looks like email.
pub fn detect(text: &str, cursor: usize) -> Option<AtContext> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let at_idx = text[..cursor].rfind('@')?;
    if let Some(ch) = text[..at_idx].chars().next_back() {
        if ch.is_alphanumeric() || ch == '_' {
            return None;
        }
    }
    let token_end = text[at_idx + 1..]
        .char_indices()
        .find_map(|(offset, ch)| {
            (ch.is_whitespace() || matches!(ch, ',' | ';')).then_some(at_idx + 1 + offset)
        })
        .unwrap_or(text.len());
    if cursor > token_end {
        return None;
    }
    Some(AtContext {
        range: at_idx..token_end,
        cursor,
        query: text[at_idx + 1..cursor].to_owned(),
    })
}

pub fn normalize_display_path(path: &str) -> &str {
    path.strip_prefix("./").unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_at_token() {
        let ctx = detect("@foo", 4).unwrap();
        assert_eq!(ctx.range, 0..4);
        assert_eq!(ctx.query, "foo");
        assert!(!ctx.is_dir_mode());
    }

    #[test]
    fn rejected_email_like() {
        assert!(detect("user@example", 12).is_none());
        assert!(detect("test_@foo", 9).is_none());
    }

    #[test]
    fn cursor_at_sign_only() {
        let ctx = detect("@", 1).unwrap();
        assert_eq!(ctx.query, "");
    }

    #[test]
    fn dir_mode() {
        let ctx = detect("@src/", 5).unwrap();
        assert!(ctx.is_dir_mode());
    }

    #[test]
    fn cursor_past_token() {
        assert!(detect("@foo bar", 5).is_none());
    }

    #[test]
    fn hidden_mode() {
        let ctx = detect("@!foo", 5).unwrap();
        assert!(ctx.is_hidden_mode());
        assert_eq!(ctx.matcher_query(), "foo");
        assert_eq!(ctx.path_range(), 2..5);
    }
}

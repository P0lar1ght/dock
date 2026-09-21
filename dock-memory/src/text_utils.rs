//! Shared classifiers for flush / dream response processing.

pub fn has_markdown_headers(text: &str) -> bool {
    text.contains("## ") || text.contains("# ")
}

/// True when the response is the NO_REPLY marker in any case or separator variant.
pub fn is_no_reply(text: &str) -> bool {
    let normalized: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    normalized == "noreply"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_no_reply() {
        assert!(is_no_reply("NO_REPLY"));
        assert!(is_no_reply("no reply"));
        assert!(is_no_reply("No-Reply"));
        assert!(!is_no_reply("no reply needed"));
    }

    #[test]
    fn test_has_markdown_headers() {
        assert!(has_markdown_headers("## Topic"));
        assert!(!has_markdown_headers("plain text"));
        assert!(!has_markdown_headers("#hashtag"));
    }
}

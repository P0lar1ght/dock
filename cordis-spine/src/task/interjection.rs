//! Grok send-now envelope. Copied from `xai-interjection-core` format, Chinese host text.

const LARGE_PROMPT_THRESHOLD: usize = 25_000;

pub fn format_interjection(text: &str) -> String {
    format_steered("主代理在你工作期间发来消息：", text)
}

fn format_steered(note: &str, text: &str) -> String {
    let truncated = if text.len() <= LARGE_PROMPT_THRESHOLD {
        text.to_string()
    } else {
        let end = text
            .char_indices()
            .take_while(|(i, _)| *i < LARGE_PROMPT_THRESHOLD)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(text.len());
        format!("{}... [truncated]", &text[..end])
    };
    format!("{note}\n<user_query>\n{truncated}\n</user_query>\n请继续完成此前未完成的工作。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_query_and_unfinished_trailer() {
        let out = format_interjection("先修测试");
        assert!(out.starts_with("主代理在你工作期间发来消息："));
        assert!(out.contains("<user_query>\n先修测试\n</user_query>"));
        assert!(out.contains("请继续完成此前未完成的工作。"));
    }
}

//! `/remember` note rewrite prompt (Grok-aligned).

pub const REMEMBER_REWRITE_SYSTEM_PROMPT: &str = "\
You are a memory note formatter. Rewrite the user's note into well-structured markdown suitable for a persistent memory observation. The note should be:\n- Concise but complete\n- Start with a descriptive ## heading\n- Include enough context to be useful months later\n- Reference specific files, decisions, or patterns when relevant\n- Use bullet points for multiple items\n- Do NOT include timestamps or session IDs\n- Do NOT add information that is not present in the original note\n\nReturn ONLY the formatted markdown, no explanations.";

pub fn rewrite_user_message(raw_text: &str, context_summary: &str) -> String {
    if context_summary.trim().is_empty() {
        format!("Rewrite this note as a memory entry:\n\n{raw_text}")
    } else {
        format!(
            "Session context:\n{context_summary}\n\nRewrite this note as a memory entry:\n\n{raw_text}"
        )
    }
}

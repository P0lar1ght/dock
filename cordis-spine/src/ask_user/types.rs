//! Copied from Grok `ask_user_question/types.rs` (`QuestionAnnotation` only).

/// Annotation on a single question's answer.
///
/// - `preview`: verbatim `Option.preview` of the selected option (single-select only).
/// - `notes`: free-text the user typed in the freeform input.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct QuestionAnnotation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

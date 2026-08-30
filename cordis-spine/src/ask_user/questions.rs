#![allow(dead_code)] // Grok-copied API kept for later wiring.

/// A single option within a question.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuestionOption {
    /// Option text shown to the user; a few words at most.
    pub label: String,
    /// What picking this option means or implies.
    #[serde(default)]
    pub description: String,
    /// Optional content shown while the option is focused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Opaque id; hidden from the model. Grok callers leave it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// A single question with its options.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    /// The question to ask, phrased as a full question.
    pub question: String,
    /// The choices for this question.
    pub options: Vec<QuestionOption>,
    /// Let the user pick more than one option (default false).
    #[serde(default, alias = "multi_select")]
    pub multi_select: Option<bool>,
    /// See `QuestionOption.id`. Hidden from the JSON schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Input for the `AskUserQuestion` tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AskUserQuestionInput {
    /// The questions to ask, each with its own options. At least one question
    /// is required.
    pub questions: Vec<Question>,
    /// Internal flag: when `true`, the tool result is formatted in the
    /// alternate shape (referenced by id, not label).
    #[serde(default, skip)]
    pub use_id_keyed_format: bool,
}

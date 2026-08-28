//! Copied from grok pager `slash/command.rs` (`ArgItem`).

/// A suggestion item for command argument completion.
#[derive(Debug, Clone)]
pub struct ArgItem {
    pub display: String,
    pub match_text: String,
    pub insert_text: String,
    pub description: String,
}

impl ArgItem {
    pub fn new(display: impl Into<String>, description: impl Into<String>) -> Self {
        let display = display.into();
        Self {
            match_text: display.clone(),
            insert_text: display.clone(),
            display,
            description: description.into(),
        }
    }
}

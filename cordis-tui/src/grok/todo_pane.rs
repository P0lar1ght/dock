//! Todo pane icons — copied from grok-build/.../xai-grok-pager/src/views/todo_pane.rs.
//! ListPane wrapper omitted; scrollback uses the same glyphs + theme colors.

use ratatui::style::{Modifier, Style};

use crate::grok::glyphs;
use crate::theme::Theme;
use cordis_spine::TodoStatus;

/// Visual style for each todo status. Copied from Grok `TodoStatusStyle`.
#[derive(Debug, Clone, Copy)]
pub struct TodoStatusStyle {
    pub icon_fg: ratatui::style::Color,
    pub text_style: Style,
}

/// Full style configuration for the todo pane. Copied from Grok `TodoPaneStyle`.
#[derive(Debug, Clone, Copy)]
pub struct TodoPaneStyle {
    pub pending: TodoStatusStyle,
    pub in_progress: TodoStatusStyle,
    pub completed: TodoStatusStyle,
    pub cancelled: TodoStatusStyle,
}

impl Default for TodoPaneStyle {
    fn default() -> Self {
        let theme = Theme::current();
        Self {
            pending: TodoStatusStyle {
                icon_fg: theme.text_primary,
                text_style: Style::default().fg(theme.text_primary),
            },
            in_progress: TodoStatusStyle {
                icon_fg: theme.warning,
                text_style: Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            },
            completed: TodoStatusStyle {
                icon_fg: theme.accent_success,
                text_style: Style::default().fg(theme.gray_bright),
            },
            cancelled: TodoStatusStyle {
                icon_fg: theme.accent_error,
                text_style: Style::default()
                    .fg(theme.gray_bright)
                    .add_modifier(Modifier::CROSSED_OUT),
            },
        }
    }
}

impl TodoPaneStyle {
    pub fn for_status(&self, status: TodoStatus) -> TodoStatusStyle {
        match status {
            TodoStatus::Pending => self.pending,
            TodoStatus::InProgress => self.in_progress,
            TodoStatus::Completed => self.completed,
            TodoStatus::Cancelled => self.cancelled,
        }
    }
}

/// Status icon for the current status. Copied from Grok `TodoListEntry::icon`.
pub fn todo_icon(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "□",
        TodoStatus::InProgress => "▶",
        TodoStatus::Completed => glyphs::check_mark(),
        TodoStatus::Cancelled => glyphs::ballot_x(),
    }
}

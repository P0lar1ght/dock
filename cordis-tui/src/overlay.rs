//! Resume / help / history / find overlays on Grok picker chrome.

use cordis_spine::ArchivedSession;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::grok::picker::{
    render_floating_frame, render_fullscreen_frame, render_picker_list, PickerHits, PickerRow,
};
use crate::settings_modal::SettingsField;
use crate::slash::{ArgKind, SlashCmd};
use crate::theme::Theme;

#[derive(Debug, Clone, Default)]
pub enum Overlay {
    #[default]
    None,
    Resume {
        selected: usize,
        query: String,
    },
    Help {
        selected: usize,
        query: String,
    },
    History {
        selected: usize,
        query: String,
    },
    Find {
        selected: usize,
        query: String,
    },
    Args {
        kind: ArgKind,
        cmd: SlashCmd,
        selected: usize,
        query: String,
    },
    Settings {
        selected: usize,
        picking: Option<SettingsField>,
    },
    Permission {
        selected: usize,
    },
    Ask {
        selected: usize,
        picked: Vec<bool>,
    },
    Tasks {
        selected: usize,
        query: String,
    },
    Mcps {
        selected: usize,
        query: String,
    },
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn close(&mut self) {
        *self = Self::None;
    }

    #[allow(dead_code)]
    pub fn query(&self) -> &str {
        match self {
            Self::None => "",
            Self::Resume { query, .. }
            | Self::Help { query, .. }
            | Self::History { query, .. }
            | Self::Find { query, .. }
            | Self::Args { query, .. } => query,
            Self::Settings { .. }
            | Self::Permission { .. }
            | Self::Ask { .. } => "",
            Self::Tasks { query, .. } | Self::Mcps { query, .. } => query,
        }
    }

    pub fn push_char(&mut self, c: char) {
        if let Some(q) = self.query_mut() {
            q.push(c);
        }
        self.set_selected(0);
    }

    pub fn push_str(&mut self, s: &str) {
        if let Some(q) = self.query_mut() {
            q.push_str(s);
        }
        self.set_selected(0);
    }

    pub fn backspace(&mut self) {
        if let Some(q) = self.query_mut() {
            q.pop();
        }
        self.set_selected(0);
    }

    pub fn move_sel(&mut self, delta: i16, len: usize) {
        if len == 0 {
            return;
        }
        let next = (self.selected() as i32 + delta as i32).rem_euclid(len as i32) as usize;
        self.set_selected(next);
    }

    pub fn selected(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Resume { selected, .. }
            | Self::Help { selected, .. }
            | Self::History { selected, .. }
            | Self::Find { selected, .. }
            | Self::Args { selected, .. }
            | Self::Settings { selected, .. }
            | Self::Permission { selected, .. }
            | Self::Ask { selected, .. }
            | Self::Tasks { selected, .. }
            | Self::Mcps { selected, .. } => *selected,
        }
    }

    pub fn set_selected(&mut self, selected: usize) {
        match self {
            Self::None => {}
            Self::Resume { selected: s, .. }
            | Self::Help { selected: s, .. }
            | Self::History { selected: s, .. }
            | Self::Find { selected: s, .. }
            | Self::Args { selected: s, .. }
            | Self::Settings { selected: s, .. }
            | Self::Permission { selected: s, .. }
            | Self::Ask { selected: s, .. }
            | Self::Tasks { selected: s, .. }
            | Self::Mcps { selected: s, .. } => *s = selected,
        }
    }

    fn query_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::None => None,
            Self::Resume { query, .. }
            | Self::Help { query, .. }
            | Self::History { query, .. }
            | Self::Find { query, .. }
            | Self::Args { query, .. }
            | Self::Tasks { query, .. }
            | Self::Mcps { query, .. } => Some(query),
            Self::Settings { .. } | Self::Permission { .. } | Self::Ask { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpKind {
    Hint,
    Slash(SlashCmd),
}

#[derive(Debug, Clone, Copy)]
pub struct HelpRow {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: HelpKind,
}

#[allow(dead_code)]
enum HelpEntry {
    Header(&'static str),
    Row(HelpRow),
}

const HELP: &[HelpEntry] = &[
    HelpEntry::Header("常用"),
    HelpEntry::Row(HelpRow {
        key: "enter",
        label: "发送",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "esc",
        label: "清空 / 取消当前轮 / 关闭",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "shift+tab",
        label: "切换模式（询问 / 始终允许）",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+x",
        label: "快捷键帮助",
        kind: HelpKind::Slash(SlashCmd::Help),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+v",
        label: "粘贴文字 / 图片",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+q",
        label: "退出",
        kind: HelpKind::Slash(SlashCmd::Quit),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+w",
        label: "新会话",
        kind: HelpKind::Slash(SlashCmd::New),
    }),
    HelpEntry::Row(HelpRow {
        key: "f2",
        label: "设置",
        kind: HelpKind::Slash(SlashCmd::Settings),
    }),
    HelpEntry::Row(HelpRow {
        key: "f3",
        label: "恢复会话",
        kind: HelpKind::Slash(SlashCmd::Resume),
    }),
    HelpEntry::Row(HelpRow {
        key: "ctrl+.",
        label: "快捷键帮助",
        kind: HelpKind::Slash(SlashCmd::Help),
    }),
    HelpEntry::Header("会话"),
    HelpEntry::Row(HelpRow {
        key: "/new",
        label: "开始新会话",
        kind: HelpKind::Slash(SlashCmd::New),
    }),
    HelpEntry::Row(HelpRow {
        key: "/resume",
        label: "恢复上次会话",
        kind: HelpKind::Slash(SlashCmd::Resume),
    }),
    HelpEntry::Row(HelpRow {
        key: "/history",
        label: "搜索提示词历史",
        kind: HelpKind::Slash(SlashCmd::History),
    }),
    HelpEntry::Row(HelpRow {
        key: "/find",
        label: "搜索对话",
        kind: HelpKind::Slash(SlashCmd::Find),
    }),
    HelpEntry::Row(HelpRow {
        key: "/copy",
        label: "复制上一条回复",
        kind: HelpKind::Slash(SlashCmd::Copy),
    }),
    HelpEntry::Row(HelpRow {
        key: "/theme",
        label: "切换配色",
        kind: HelpKind::Slash(SlashCmd::Theme),
    }),
    HelpEntry::Row(HelpRow {
        key: "/timestamps",
        label: "开关时间戳",
        kind: HelpKind::Slash(SlashCmd::Timestamps),
    }),
    HelpEntry::Row(HelpRow {
        key: "/model",
        label: "切换模型",
        kind: HelpKind::Slash(SlashCmd::Model),
    }),
    HelpEntry::Row(HelpRow {
        key: "/settings",
        label: "设置",
        kind: HelpKind::Slash(SlashCmd::Settings),
    }),
    HelpEntry::Row(HelpRow {
        key: "/loop",
        label: "按间隔循环提问",
        kind: HelpKind::Slash(SlashCmd::Loop),
    }),
    HelpEntry::Row(HelpRow {
        key: "/plan",
        label: "进入计划模式",
        kind: HelpKind::Slash(SlashCmd::Plan),
    }),
    HelpEntry::Row(HelpRow {
        key: "/goal",
        label: "开始或查看目标",
        kind: HelpKind::Slash(SlashCmd::Goal),
    }),
    HelpEntry::Row(HelpRow {
        key: "/tasks",
        label: "后台任务与定时任务",
        kind: HelpKind::Slash(SlashCmd::Tasks),
    }),
    HelpEntry::Row(HelpRow {
        key: "/mcps",
        label: "MCP 服务器状态",
        kind: HelpKind::Slash(SlashCmd::Mcps),
    }),
    HelpEntry::Row(HelpRow {
        key: "/quit",
        label: "退出",
        kind: HelpKind::Slash(SlashCmd::Quit),
    }),
    HelpEntry::Header("输入"),
    HelpEntry::Row(HelpRow {
        key: "↑↓",
        label: "翻历史提示词",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "←→",
        label: "移动光标",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "shift+enter",
        label: "换行",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "@",
        label: "搜索文件",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "tab",
        label: "补全斜杠命令或路径",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Header("对话"),
    HelpEntry::Row(HelpRow {
        key: "pgup",
        label: "滚动对话",
        kind: HelpKind::Hint,
    }),
    HelpEntry::Row(HelpRow {
        key: "click",
        label: "展开工具 / 复制 mermaid",
        kind: HelpKind::Hint,
    }),
];

pub fn matches_query(hay: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    hay.to_ascii_lowercase()
        .contains(&query.to_ascii_lowercase())
}

pub fn filter_sessions<'a>(items: &'a [ArchivedSession], query: &str) -> Vec<&'a ArchivedSession> {
    items
        .iter()
        .filter(|s| matches_query(&s.title, query) || matches_query(&s.id, query))
        .collect()
}

pub fn filter_strings<'a>(items: &'a [String], query: &str) -> Vec<(usize, &'a str)> {
    items
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, s)| matches_query(s, query))
        .map(|(i, s)| (i, s.as_str()))
        .collect()
}

pub fn filter_help(query: &str) -> Vec<&'static HelpRow> {
    HELP.iter()
        .filter_map(|e| match e {
            HelpEntry::Row(row)
                if matches_query(row.key, query) || matches_query(row.label, query) =>
            {
                Some(row)
            }
            _ => None,
        })
        .collect()
}

pub fn help_at(query: &str, selected: usize) -> Option<&'static HelpRow> {
    filter_help(query).get(selected).copied()
}

/// `fullscreen` = welcome-screen session picker (Grok `PickerMode::FullScreen`).
pub fn render_overlay(
    buf: &mut Buffer,
    area: Rect,
    title: &str,
    query: &str,
    rows: &[PickerRow<'_>],
    fullscreen: bool,
) -> PickerHits {
    let theme = Theme::current();
    let frame = if fullscreen {
        render_fullscreen_frame(buf, area, &theme, Some(title), false)
    } else {
        render_floating_frame(buf, area, &theme, false)
    };
    let Some(frame) = frame else {
        return PickerHits::default();
    };
    let skip_top = if fullscreen { 0 } else { 1 };
    let row_hits = render_picker_list(buf, frame.content, &theme, query, rows, skip_top);
    PickerHits {
        close_button: frame.close_button,
        rows: row_hits,
    }
}

pub fn hit_index(hits: &PickerHits, column: u16, row: u16) -> Option<usize> {
    let pos = Position {
        x: column,
        y: row,
    };
    if hits.close_button.contains(pos) {
        return None;
    }
    hits.rows
        .iter()
        .find(|(_, r)| r.contains(pos))
        .map(|(i, _)| *i)
}

pub fn hit_close(hits: &PickerHits, column: u16, row: u16) -> bool {
    hits.close_button.contains(Position {
        x: column,
        y: row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_case_insensitive() {
        assert!(matches_query("Hello World", "hello"));
        assert!(!matches_query("Hello World", "zzz"));
    }

    #[test]
    fn help_filter_finds_resume() {
        let rows = filter_help("resume");
        assert!(rows.iter().any(|r| r.key.contains("resume") || r.label.contains("Resume")));
    }

    #[test]
    fn help_lists_shift_tab_mode_and_ctrl_x() {
        let rows = filter_help("");
        assert!(
            rows.iter().any(|r| r.key == "shift+tab"),
            "{:?}",
            rows.iter().map(|r| r.key).collect::<Vec<_>>()
        );
        assert!(rows.iter().any(|r| r.key == "ctrl+x"));
        assert!(rows.iter().any(|r| r.label.contains("询问")));
        assert!(rows.iter().any(|r| r.key == "/plan"));
        assert!(rows.iter().any(|r| r.key == "/goal"));
        assert!(rows.iter().any(|r| r.key == "/tasks"));
        assert!(rows.iter().any(|r| r.key == "/mcps"));
    }
}

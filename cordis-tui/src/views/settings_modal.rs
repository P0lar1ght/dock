//! Grok settings modal chrome (category headers, value column, enum picker).
//! Registry is dock's live `"settings"` — not Grok `UiConfig`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::picker::{render_floating_frame, PickerHits, PickerRow};
use crate::theme::{Theme, ThemeKind};
use crate::views::overlay;
use cordis_spine::{AppSettings, MermaidEngineKind, PermissionMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Theme,
    Timestamps,
    Model,
    Thinking,
    Effort,
    PermissionMode,
    MermaidEngine,
}

#[derive(Debug, Clone, Copy)]
pub enum SettingsRow {
    Header(&'static str),
    Field(SettingsField),
}

pub const ROWS: &[SettingsRow] = &[
    SettingsRow::Header("外观"),
    SettingsRow::Field(SettingsField::Theme),
    SettingsRow::Field(SettingsField::Timestamps),
    SettingsRow::Header("模型"),
    SettingsRow::Field(SettingsField::Model),
    SettingsRow::Field(SettingsField::Thinking),
    SettingsRow::Field(SettingsField::Effort),
    SettingsRow::Header("工具"),
    SettingsRow::Field(SettingsField::PermissionMode),
    SettingsRow::Field(SettingsField::MermaidEngine),
];

pub fn field_rows() -> Vec<SettingsField> {
    ROWS.iter()
        .filter_map(|r| match r {
            SettingsRow::Field(f) => Some(*f),
            SettingsRow::Header(_) => None,
        })
        .collect()
}

impl SettingsField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "主题",
            Self::Timestamps => "时间戳",
            Self::Model => "模型",
            Self::Thinking => "思考模式",
            Self::Effort => "推理强度",
            Self::PermissionMode => "权限",
            Self::MermaidEngine => "Mermaid 引擎",
        }
    }

    pub fn is_bool(self) -> bool {
        matches!(self, Self::Timestamps | Self::Thinking)
    }
}

pub fn value_text(field: SettingsField, settings: &AppSettings) -> String {
    match field {
        SettingsField::Theme => Theme::current_kind().display_name().into(),
        SettingsField::Timestamps => bool_label(settings.timestamps()),
        SettingsField::Thinking => {
            if settings.reasoning_available() {
                bool_label(settings.thinking())
            } else {
                "（该模型无推理档）".into()
            }
        }
        SettingsField::Model => {
            let model = settings.model();
            if model.is_empty() {
                "（未配置，见 config.toml）".into()
            } else {
                model
            }
        }
        SettingsField::Effort => {
            if !settings.reasoning_available() {
                "（该模型无推理档）".into()
            } else if !settings.thinking() {
                "（思考已关）".into()
            } else if settings.effort().is_empty() {
                "（默认）".into()
            } else {
                settings.effort()
            }
        }
        SettingsField::PermissionMode => match settings.permission_mode() {
            PermissionMode::Ask => "询问".into(),
            PermissionMode::Allow => "自动允许".into(),
        },
        SettingsField::MermaidEngine => match settings.mermaid_engine() {
            MermaidEngineKind::Pure => "pure".into(),
            MermaidEngineKind::Mmdc => "mmdc".into(),
        },
    }
}

fn bool_label(on: bool) -> String {
    if on {
        "开".into()
    } else {
        "关".into()
    }
}

pub fn enum_choices(field: SettingsField, settings: &AppSettings) -> Vec<(String, String)> {
    match field {
        SettingsField::Theme => ThemeKind::ALL
            .iter()
            .map(|k| (k.display_name().to_string(), "配色".into()))
            .collect(),
        SettingsField::Model => settings
            .catalog()
            .into_iter()
            .map(|m| {
                let desc = if m.description.is_empty() {
                    m.name
                } else {
                    m.description
                };
                (m.id, desc)
            })
            .collect(),
        // 档位来自当前模型的 reasoning_efforts，没配才是通用四档。
        SettingsField::Effort => settings
            .effort_choices()
            .into_iter()
            .map(|e| (e, "推理".into()))
            .collect(),
        SettingsField::PermissionMode => vec![
            ("询问".into(), "每次需确认".into()),
            ("自动允许".into(), "不再询问".into()),
        ],
        SettingsField::MermaidEngine => vec![
            ("pure".into(), "内置 dagre".into()),
            ("mmdc".into(), "PATH 上的 mermaid-cli".into()),
        ],
        SettingsField::Timestamps | SettingsField::Thinking => Vec::new(),
    }
}

pub fn apply_choice(field: SettingsField, value: &str, settings: &AppSettings) {
    match field {
        SettingsField::Theme => {
            if let Some(kind) = ThemeKind::from_name(value) {
                Theme::apply_kind(kind);
            }
        }
        SettingsField::Model => settings.set_model(value),
        SettingsField::Effort => settings.set_effort(value),
        SettingsField::PermissionMode => {
            let mode = if value == "allow" || value == "自动允许" {
                PermissionMode::Allow
            } else {
                PermissionMode::Ask
            };
            settings.set_permission_mode(mode);
        }
        SettingsField::MermaidEngine => {
            let kind = if value == "mmdc" {
                MermaidEngineKind::Mmdc
            } else {
                MermaidEngineKind::Pure
            };
            settings.set_mermaid_engine(kind);
        }
        SettingsField::Timestamps | SettingsField::Thinking => {}
    }
}

pub fn toggle_bool(field: SettingsField, settings: &AppSettings) {
    match field {
        SettingsField::Timestamps => {
            settings.toggle_timestamps();
        }
        SettingsField::Thinking => {
            settings.toggle_thinking();
        }
        _ => {}
    }
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    selected: usize,
    picking: Option<SettingsField>,
    settings: &AppSettings,
) -> PickerHits {
    if let Some(field) = picking {
        let choices = enum_choices(field, settings);
        let sel = if choices.is_empty() {
            0
        } else {
            selected.min(choices.len().saturating_sub(1))
        };
        let rows: Vec<PickerRow> = choices
            .iter()
            .enumerate()
            .map(|(i, (label, desc))| PickerRow {
                label: label.as_str(),
                right_label: desc.as_str(),
                selected: i == sel,
            })
            .collect();
        return overlay::render_overlay(buf, area, field.label(), "", &rows, false);
    }
    render_settings_rows(buf, area, selected, settings)
}

fn render_settings_rows(
    buf: &mut Buffer,
    area: Rect,
    selected: usize,
    settings: &AppSettings,
) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };
    let fields = field_rows();
    let sel_field = fields.get(selected).copied();
    let mut y = frame.content.y;
    let mut hits = PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    };
    let mut field_i = 0usize;
    for row in ROWS {
        if y >= frame.content.y + frame.content.height {
            break;
        }
        match row {
            SettingsRow::Header(title) => {
                let style = Style::default().fg(theme.gray).add_modifier(Modifier::BOLD);
                buf.set_line(
                    frame.content.x,
                    y,
                    &Line::from(Span::styled((*title).to_string(), style)),
                    frame.content.width,
                );
                y = y.saturating_add(1);
            }
            SettingsRow::Field(f) => {
                let selected = sel_field == Some(*f);
                let bg = if selected {
                    theme.bg_visual
                } else {
                    theme.bg_light
                };
                let rect = Rect {
                    x: frame.content.x,
                    y,
                    width: frame.content.width,
                    height: 1,
                };
                buf.set_style(rect, Style::default().bg(bg));
                let label_style =
                    Style::default()
                        .fg(theme.text_primary)
                        .bg(bg)
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        });
                let val = value_text(*f, settings);
                buf.set_line(
                    rect.x + 1,
                    y,
                    &Line::from(Span::styled(f.label().to_string(), label_style)),
                    rect.width.saturating_sub(2),
                );
                let vw = unicode_width::UnicodeWidthStr::width(val.as_str()) as u16;
                let vx = rect.x + rect.width.saturating_sub(vw + 1);
                buf.set_line(
                    vx,
                    y,
                    &Line::from(Span::styled(
                        val,
                        Style::default().fg(theme.gray_bright).bg(bg),
                    )),
                    vw + 1,
                );
                hits.rows.push((field_i, rect));
                field_i += 1;
                y = y.saturating_add(1);
            }
        }
    }
    hits
}

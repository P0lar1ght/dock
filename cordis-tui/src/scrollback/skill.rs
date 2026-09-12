//! `skill` tool card — loading a `SKILL.md` body into the turn.
//!
//! Deliberately not part of [`super::task_ops`]: `skill` is a standalone tool
//! (no target id, no child overlay, no job snapshot), it only reports the
//! loaded envelope `<skill name="…" path="…">`. Keeping it here stops
//! `is_task_op` from claiming it.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::theme::Theme;

pub fn is_skill(name: &str) -> bool {
    name == "skill"
}

/// Status verb + skill name + source layer (+ args) + body preview.
pub fn lines(arguments: &str, content: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let pending = content.trim().is_empty();
    let err = content.trim_start().starts_with("Error");
    let running = pending && !err;
    let verb = if err {
        "加载失败"
    } else if running {
        "加载技能"
    } else {
        "已加载技能"
    };
    let label = json_str(arguments, "name");
    let meta = (!err && !pending)
        .then(|| skill_meta(content, arguments))
        .flatten();

    let label_style = if running {
        theme.primary().add_modifier(Modifier::BOLD)
    } else {
        theme.muted().add_modifier(Modifier::BOLD)
    };
    let body_style = if running {
        theme.primary()
    } else {
        theme.muted()
    };

    let mut spans = vec![Span::styled(verb.to_string(), label_style)];
    if let Some(label) = &label {
        spans.push(Span::styled(
            format!(" \u{201c}{label}\u{201d}"),
            body_style,
        ));
    }
    if let Some(meta) = &meta {
        spans.push(Span::styled(format!(" \u{00b7} {meta}"), theme.muted()));
    }

    let mut line = Line::from(spans);
    let accent = if err {
        theme.accent_error
    } else if running {
        theme.accent_running
    } else {
        theme.accent_thinking
    };
    line.spans.insert(0, live::diamond(accent));
    if width > 0 {
        line = truncate_line(line, width);
    }

    let mut out = vec![line];
    // 技能卡没有可跳的 overlay，也没有 elapsed；正文预览只在结果落地后出现。
    if !running {
        if let Some(preview) = preview(content) {
            let mut prev = card::indent(Line::from(Span::styled(preview, theme.dim())));
            if width > 0 {
                prev = truncate_line(prev, width);
            }
            out.push(prev);
        }
    }
    out
}

fn json_str(arguments: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `<skill …>` 信封的开合标签不算正文。
fn preview(content: &str) -> Option<String> {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('<'))
        .map(first_line)
}

/// First non-empty line, capped so the preview never wraps.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('\u{2026}');
    }
    t
}

/// 技能结果信封里的来源层 + 用户传入的 args：
/// `bundled/skills/…` → 内置，`skills/…` → 仓库，`~/.dock/skills/…` → 用户，
/// `.agents/skills/…` → agents，`.dock/skills/…` → 项目。
fn skill_meta(content: &str, arguments: &str) -> Option<String> {
    let header = content.lines().next()?;
    let path = attr(header, "path=\"")?;
    let scope = if path.starts_with("bundled/") {
        "内置"
    } else if path.starts_with("skills/") {
        "仓库"
    } else if path.starts_with("~/.dock/") {
        "用户"
    } else if path.starts_with(".agents/") {
        "agents"
    } else if path.starts_with(".dock/") {
        "项目"
    } else {
        "未知来源"
    };
    let args = json_str(arguments, "args").unwrap_or_default();
    Some(if args.trim().is_empty() {
        scope.to_string()
    } else {
        format!("{scope} · args={args}")
    })
}

/// 从 `… path="…"` 形式的属性里取值。
fn attr(header: &str, key: &str) -> Option<String> {
    let rest = header.split_once(key)?.1;
    let value = rest.split('"').next()?;
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn skill_card_shows_name_and_body_preview() {
        let theme = Theme::current();
        let content = "<skill name=\"dock-config\" path=\"bundled/skills/dock-config/SKILL.md\">\n\
                       # Dock 配置手册\n\n配置按顺序合并……\n</skill>\n\nFiles in skill dir:\n";
        let card = lines(r#"{"name":"dock-config"}"#, content, &theme, 120);
        let text = flat(&card);
        assert!(text.contains("已加载技能"), "{text}");
        assert!(text.contains("dock-config"), "{text}");
        assert!(text.contains("Dock 配置手册"), "{text}");
        assert!(
            !text.contains("点击查看"),
            "skill 卡片无 overlay 可跳：{text}"
        );

        let pending = lines(r#"{"name":"dock-config"}"#, "", &theme, 120);
        assert!(flat(&pending).contains("加载技能"));

        let failed = lines(
            r#"{"name":"nope"}"#,
            "Error: unknown skill \"nope\"",
            &theme,
            120,
        );
        assert!(flat(&failed).contains("加载失败"));
    }

    #[test]
    fn skill_card_shows_source_layer_and_args() {
        let theme = Theme::current();
        let env =
            |path: &str| format!("<skill name=\"dock-config\" path=\"{path}\">\n# 正文\n</skill>");
        let cases = [
            ("bundled/skills/dock-config/SKILL.md", "内置"),
            ("skills/mcp-builder/SKILL.md", "仓库"),
            ("~/.dock/skills/my-skill/SKILL.md", "用户"),
            (".agents/skills/x/SKILL.md", "agents"),
            (".dock/skills/y/SKILL.md", "项目"),
        ];
        for (path, label) in cases {
            let card = lines(r#"{"name":"dock-config"}"#, &env(path), &theme, 120);
            let text = flat(&card);
            assert!(text.contains("已加载技能"), "{text}");
            assert!(text.contains(label), "{path} 应显示 {label}: {text}");
        }

        let with_args = lines(
            r#"{"name":"dock-config","args":"mcp 怎么接"}"#,
            &env("bundled/skills/dock-config/SKILL.md"),
            &theme,
            120,
        );
        let text = flat(&with_args);
        assert!(text.contains("内置"), "{text}");
        assert!(text.contains("args=mcp 怎么接"), "{text}");
    }

    /// deferred 包装的 `skill` 必须被 dispatch 解包成本卡（见 `scrollback::mod`）。
    #[test]
    fn use_tool_wrapped_skill_renders_this_card() {
        let theme = Theme::current();
        let (inner, inner_args) = crate::scrollback::unwrap_use_tool(
            "use_tool",
            r#"{"tool_name":"skill","tool_input":{"name":"dock-config"}}"#,
        )
        .expect("skill is unwrapped");
        assert_eq!(inner, "skill");
        assert!(is_skill(&inner));
        let card = lines(
            &inner_args,
            "<skill name=\"dock-config\" path=\"bundled/skills/dock-config/SKILL.md\">\n# Dock 配置手册\n</skill>",
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("已加载技能"), "{text}");
        assert!(text.contains("dock-config"), "{text}");
    }
}

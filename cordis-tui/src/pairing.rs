//! Named `"tui.pairing"`: pending Origin overlay + `/pair` management.
//! Live-look `"gateway"`. Pairing state stays on the gateway plugin.

use std::time::{Duration, SystemTime};

use cordis::Context;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthChar;

use crate::gateway::{GatewayRef, PairingBinding, PairingPrompt};
use crate::grok::glyphs;
use crate::grok::picker::{PickerHits, PickerRow};
use crate::names::GATEWAY;
use crate::overlay::{self, Overlay};
use crate::theme::Theme;

pub const PENDING_OPTIONS: &[&str] = &["批准", "拒绝"];

pub struct PairingUi {
    ctx: Context,
}

impl PairingUi {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    pub fn pending(&self) -> Vec<PairingPrompt> {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .map(|g| g.pairing_pending())
            .unwrap_or_default()
    }

    pub fn bindings(&self) -> Vec<PairingBinding> {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .map(|g| g.pairing_bindings())
            .unwrap_or_default()
    }

    pub fn confirm(&self, id: &str) -> bool {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .and_then(|g| g.pairing_confirm(id).ok())
            .is_some()
    }

    pub fn deny(&self, id: &str) -> bool {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .and_then(|g| g.pairing_deny(id).ok())
            .is_some()
    }

    pub fn revoke(&self, origin: &str) -> bool {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .and_then(|g| g.pairing_revoke(origin).ok())
            .is_some()
    }

    pub fn is_listening(&self) -> bool {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .map(|g| g.is_listening())
            .unwrap_or(false)
    }

    pub fn start_listen(&self) -> bool {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .and_then(|g| g.start_listen().ok())
            .is_some()
    }

    pub fn stop_listen(&self) -> bool {
        self.ctx
            .get::<GatewayRef>(GATEWAY)
            .and_then(|g| g.stop_listen().ok())
            .is_some()
    }

    pub fn toggle_listen(&self) -> bool {
        if self.is_listening() {
            self.stop_listen()
        } else {
            self.start_listen()
        }
    }
}

pub fn chrome_height(prompt: &PairingPrompt, width: u16) -> u16 {
    let content_w = width.saturating_sub(5).max(8) as usize;
    let origin_rows = wrap_line(&prompt.origin, content_w).len().min(2) as u16;
    (6 + origin_rows + PENDING_OPTIONS.len() as u16).clamp(12, 20)
}

pub fn render_pending(
    buf: &mut Buffer,
    area: Rect,
    prompt: &PairingPrompt,
    selected: usize,
) -> PickerHits {
    if area.height == 0 || area.width == 0 {
        return PickerHits::default();
    }
    let theme = Theme::current();
    let bg = Style::default().fg(theme.text_primary).bg(theme.bg_light);
    Clear.render(area, buf);
    fill_rect(buf, area, bg);

    let accent = Style::default().fg(theme.accent_user).bg(theme.bg_light);
    for row in area.y..area.y + area.height {
        if let Some(cell) = buf.cell_mut((area.x, row)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(accent);
        }
    }

    let content_x = area.x.saturating_add(3);
    let content_w = area.width.saturating_sub(5);
    let mut y = area.y.saturating_add(1);
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_light)
        .add_modifier(Modifier::BOLD);
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled("允许浏览器连接？", title_style)),
        content_w,
    );
    y = y.saturating_add(1);

    let meta = Style::default().fg(theme.gray).bg(theme.bg_light);
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(format!("应用  {}", prompt.application), meta)),
        content_w,
    );
    y = y.saturating_add(1);
    for line in wrap_line(&format!("来源  {}", prompt.origin), content_w as usize)
        .into_iter()
        .take(2)
    {
        if y >= area.y + area.height {
            break;
        }
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled(line, meta)),
            content_w,
        );
        y = y.saturating_add(1);
    }
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(
            format!(
                "请求  {}  ·  过期  {}",
                format_time(prompt.created_at),
                format_time(prompt.expires_at)
            ),
            meta,
        )),
        content_w,
    );
    y = y.saturating_add(2);

    let sel = selected.min(PENDING_OPTIONS.len().saturating_sub(1));
    let mut hits = PickerHits::default();
    for (i, label) in PENDING_OPTIONS.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }
        let selected = i == sel;
        let row_bg = if selected {
            theme.bg_visual
        } else {
            theme.bg_light
        };
        let row_rect = Rect {
            x: area.x.saturating_add(1),
            y,
            width: area.width.saturating_sub(1),
            height: 1,
        };
        fill_rect(buf, row_rect, Style::default().bg(row_bg));
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(Style::default().fg(theme.accent_user).bg(row_bg));
        }
        let marker = if selected {
            glyphs::filled_dot()
        } else {
            glyphs::hollow_dot()
        };
        let text_style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary).bg(row_bg)
        };
        buf.set_line(
            content_x,
            y,
            &Line::from(vec![
                Span::styled(
                    format!("{} ", i + 1),
                    Style::default().fg(theme.accent_user).bg(row_bg),
                ),
                Span::styled(format!("{marker} {label}"), text_style),
            ]),
            content_w,
        );
        hits.rows.push((i, row_rect));
        y = y.saturating_add(1);
    }
    hits
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManageRow {
    Listen,
    Pending(usize),
    Binding(usize),
}

pub fn manage_rows(pending: &[PairingPrompt], bindings: &[PairingBinding]) -> Vec<ManageRow> {
    let mut rows = vec![ManageRow::Listen];
    for i in 0..pending.len() {
        rows.push(ManageRow::Pending(i));
    }
    for i in 0..bindings.len() {
        rows.push(ManageRow::Binding(i));
    }
    rows
}

pub fn overlay_len(pending: &[PairingPrompt], bindings: &[PairingBinding]) -> usize {
    manage_rows(pending, bindings).len()
}

pub fn overlay_copy(gw: Option<&GatewayRef>) -> (String, &'static str) {
    let empty = "没有待批请求，也没有已绑定的来源。";
    match gw {
        None => ("未挂载回环网关。".into(), "网关插件不在树上。"),
        Some(g) if !g.is_listening() => {
            let mut status = format!("未监听 {}", g.local_addr());
            status.push_str(" · Enter 开启（占用则换端口）");
            if let Some(w) = g.companion_status().warning() {
                status.push_str(" · ");
                status.push_str(&w);
            }
            (status, empty)
        }
        Some(g) => {
            let mut status = format!("监听 {}", g.local_addr());
            if let Some(w) = g.companion_status().warning() {
                status.push_str(" · ");
                status.push_str(&w);
            }
            (status, empty)
        }
    }
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
pub fn render_manage(
    buf: &mut Buffer,
    area: Rect,
    pending: &[PairingPrompt],
    bindings: &[PairingBinding],
    selected: usize,
    warning: Option<&str>,
    empty: &str,
    listening: bool,
    listen_addr: &str,
) -> PickerHits {
    let rows = manage_rows(pending, bindings);
    let owned: Vec<(String, String, bool)> = if rows.is_empty() {
        vec![(empty.into(), String::new(), true)]
    } else {
        rows.iter()
            .enumerate()
            .map(|(i, row)| {
                let (label, right) = match row {
                    ManageRow::Listen => {
                        if listening {
                            ("关闭回环网关".into(), listen_addr.to_string())
                        } else {
                            ("开启回环网关".into(), "Enter 开启".into())
                        }
                    }
                    ManageRow::Pending(j) => {
                        let p = &pending[*j];
                        (
                            format!("待批  {}  {}", p.application, p.origin),
                            format!("过期 {}", format_time(p.expires_at)),
                        )
                    }
                    ManageRow::Binding(j) => {
                        let b = &bindings[*j];
                        (
                            format!("已绑  {}  {}", b.application, b.origin),
                            format_time(b.bound_at),
                        )
                    }
                };
                (label, right, i == selected)
            })
            .collect()
    };
    let picker: Vec<PickerRow> = owned
        .iter()
        .map(|(label, right, selected)| PickerRow {
            label,
            right_label: right,
            selected: *selected,
        })
        .collect();
    overlay::render_overlay(
        buf,
        area,
        "浏览器配对",
        warning.unwrap_or(""),
        &picker,
        false,
    )
}

pub fn accept_manage(ui: &PairingUi, overlay: &mut Overlay) {
    let Overlay::PairingManage { selected } = overlay else {
        return;
    };
    let pending = ui.pending();
    let bindings = ui.bindings();
    let rows = manage_rows(&pending, &bindings);
    match rows.get(*selected) {
        Some(ManageRow::Listen) => {
            ui.toggle_listen();
        }
        Some(ManageRow::Pending(i)) => {
            if let Some(p) = pending.get(*i) {
                ui.confirm(&p.id);
            }
            *selected = 0;
        }
        Some(ManageRow::Binding(i)) => {
            if let Some(b) = bindings.get(*i) {
                ui.revoke(&b.origin);
            }
            *selected = 0;
        }
        None => {}
    }
}

pub fn reject_manage(ui: &PairingUi, overlay: &mut Overlay) {
    let Overlay::PairingManage { selected } = overlay else {
        return;
    };
    let pending = ui.pending();
    let bindings = ui.bindings();
    let rows = manage_rows(&pending, &bindings);
    match rows.get(*selected) {
        // 仅在正在监听时才停：guard 不成立时落到下面的空分支（行为与原先的内层 if 一致）。
        Some(ManageRow::Listen) if ui.is_listening() => {
            ui.stop_listen();
        }
        Some(ManageRow::Listen) => {}
        Some(ManageRow::Pending(i)) => {
            if let Some(p) = pending.get(*i) {
                ui.deny(&p.id);
            }
            *selected = 0;
        }
        Some(ManageRow::Binding(i)) => {
            if let Some(b) = bindings.get(*i) {
                ui.revoke(&b.origin);
            }
            *selected = 0;
        }
        None => {}
    }
}

fn format_time(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "?".into())
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut w = 0usize;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        if w + cw > width && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(ch);
        w += cw;
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn fill_rect(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(style);
                if cell.symbol() == " " || cell.symbol().is_empty() {
                    cell.set_symbol(" ");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_row_is_always_first() {
        assert_eq!(manage_rows(&[], &[]), vec![ManageRow::Listen]);
        assert_eq!(overlay_len(&[], &[]), 1);
        let pending = [PairingPrompt {
            id: "1".into(),
            application: "app".into(),
            origin: "http://localhost".into(),
            created_at: SystemTime::UNIX_EPOCH,
            expires_at: SystemTime::UNIX_EPOCH,
        }];
        assert_eq!(
            manage_rows(&pending, &[]),
            vec![ManageRow::Listen, ManageRow::Pending(0)]
        );
    }
}

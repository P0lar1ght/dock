//! Prompt chrome copied from grok-build/.../prompt_widget/mod.rs `draw`
//! (`╭──────────╮` / `│` / `╰─ model · flags ─╯`, flags right-aligned).
//!
//! Composer grows with content (Grok `desired_height`). Cursor editing,
//! Shift+Enter newline, bracketed paste, and `@` file search sit on the
//! pager's core input path.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Widget;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use cordis::Context;
use cordis_spine::{AppSettings, Slash, SlashEntry, SETTINGS, SLASH};

use crate::app::clipboard::ClipboardImage;
use crate::file_search::{self, FileSearchSnapshot};
use crate::theme::Theme;

const IMAGE_CAP: usize = 10;
const PASTE_CHIP_LINES: usize = 4;
/// Grok `PromptStyle` default placeholder (`placeholder_override` unset).
const PLACEHOLDER: &str = "Build anything";

#[derive(Clone, Debug)]
pub struct PastedImage {
    pub n: u32,
    pub mime: String,
    pub data: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl PastedImage {
    pub fn from_clipboard(n: u32, img: ClipboardImage) -> Self {
        Self {
            n,
            mime: img.mime,
            data: img.data,
            width: img.width,
            height: img.height,
        }
    }

    pub fn chip(&self) -> String {
        format!("[Image #{}]", self.n)
    }

    pub fn format_name(&self) -> &str {
        match self.mime.as_str() {
            "image/png" => "PNG",
            "image/jpeg" => "JPEG",
            "image/tiff" => "TIFF",
            "image/gif" => "GIF",
            "image/webp" => "WEBP",
            other => other,
        }
    }

    pub fn size_label(&self) -> String {
        format_bytes(self.data.len())
    }
}

pub struct TakenPrompt {
    pub text: String,
    pub images: Vec<PastedImage>,
}

#[derive(Default)]
struct State {
    input: String,
    /// Byte index into `input`.
    cursor: usize,
    history: Vec<String>,
    /// `None` = live buffer; `Some(i)` = recalling `history[i]`.
    history_idx: Option<usize>,
    stash: String,
    /// 斜杠下拉的高亮行。`None` = 用户还没上下选过（打字会把它打回 None），
    /// 此时补参数阶段的 Enter 照旧直接发。
    slash_selected: Option<usize>,
    file_selected: usize,
    file_dismissed: bool,
    last_at_query: String,
    chrome_info: String,
    /// Grok `PromptStyle.focused`. Empty composer during a turn is unfocused
    /// (`Build anything` placeholder, no cursor) so Enter does not steal the run.
    unfocused: bool,
    /// 上一帧回合是否在跑。只用来认**边沿**：起一轮时把空框失焦、收一轮时还回
    /// 焦点。每帧照着 `working` 重设焦点会把鼠标点击、打字的意图当场盖掉。
    turn_working: bool,
    images: Vec<PastedImage>,
    image_counter: u32,
    paste_bodies: Vec<(String, String)>,
    /// Last submitted composer text (Grok `in_flight_prompt`), for Esc rewind.
    last_sent: Option<String>,
    /// 鼠标框选：`(anchor, head)`，都是 `input` 的字节下标。`None` = 没选区。
    ///
    /// 输入框自己做选区，是因为**全屏应用把终端的原生拖选顶掉了**：开了
    /// alt-screen + 鼠标上报之后，拖动事件进的是 dock，终端那层选不了。滚动区
    /// 早就这么干了（`Scrollback::mouse_down/drag/up`），输入框一直漏着。
    selection: Option<(usize, usize)>,
    /// 按下了但还没拖过阈值：还不算选区，抬手就是普通点击（移光标）。
    pending: Option<(u16, u16, usize)>,
    dragging: bool,
    /// 上一帧画在哪儿。鼠标坐标要按它换算成字节下标，所以渲染时记一份。
    last_area: Rect,
}

impl State {
    fn clamp_cursor(&mut self) {
        if self.cursor > self.input.len() {
            self.cursor = self.input.len();
        } else if !self.input.is_char_boundary(self.cursor) {
            self.cursor = prev_boundary(&self.input, self.cursor);
        }
    }

    /// 选区正规化成 `input` 的一段字节范围；空选区返回 `None`。
    ///
    /// 也顺手挡住**过期**的选区：整段文本被换掉（历史、`/` 补全、清空）之后旧
    /// 下标可能已经越界，这里判一次比在每个改文本的地方各清一次可靠。
    fn selection_range(&self) -> Option<Range<usize>> {
        let (a, b) = self.selection?;
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if lo >= hi || hi > self.input.len() {
            return None;
        }
        (self.input.is_char_boundary(lo) && self.input.is_char_boundary(hi)).then_some(lo..hi)
    }

    /// 有选区就把它删掉并返回 `true`。
    ///
    /// 编辑动作都先过这一关：**选中之后按删除要删掉整段**，而不是再退一个字符；
    /// 打字同理，是替换而不是插在中间。
    fn take_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            self.drop_selection();
            return false;
        };
        let start = range.start;
        self.input.replace_range(range, "");
        self.drop_selection();
        self.cursor = start;
        self.clamp_cursor();
        self.slash_selected = None;
        self.file_dismissed = false;
        true
    }

    fn drop_selection(&mut self) {
        self.selection = None;
        self.pending = None;
        self.dragging = false;
    }

    /// 只挪光标、**不动选区**。鼠标拖选专用：拖的过程中光标跟着走，但选区正在
    /// 被这次拖动建立，不能被 [`Self::set_cursor`] 顺手清掉。
    fn set_cursor_keep_selection(&mut self, i: usize) {
        self.cursor = i.min(self.input.len());
        self.clamp_cursor();
        self.file_dismissed = false;
    }

    fn selected_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        self.input.get(range).map(str::to_string)
    }

    /// 挪光标即作废选区——键盘一动，鼠标框出来的那块高亮就不该再留着。
    /// 所有 `move_*` / 历史 / 补全都走这里，不必各记各的。
    fn set_cursor(&mut self, i: usize) {
        self.drop_selection();
        self.set_cursor_keep_selection(i);
    }
}

#[derive(Default)]
pub struct PromptWidget {
    ctx: Option<Context>,
    state: Mutex<State>,
}

impl PromptWidget {
    pub fn with_context(ctx: Context) -> Self {
        Self {
            ctx: Some(ctx),
            state: Mutex::new(State::default()),
        }
    }

    pub fn slash_extras(&self) -> Vec<SlashEntry> {
        self.ctx
            .as_ref()
            .and_then(|c| c.get::<Slash>(SLASH))
            .map(|s| s.list())
            .unwrap_or_default()
    }

    pub fn text(&self) -> String {
        self.state.lock().unwrap().input.clone()
    }

    pub fn can_send(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.input.trim().is_empty() || !state.images.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.state.lock().unwrap().cursor
    }

    pub fn push(&self, c: char) {
        self.insert_str(&c.to_string());
    }

    pub fn insert_str(&self, s: &str) {
        let mut state = self.state.lock().unwrap();
        insert_at_cursor(&mut state, s);
    }

    pub fn set_info(&self, info: impl Into<String>) {
        self.state.lock().unwrap().chrome_info = info.into();
    }

    /// Grok agent-view `prompt_focused`。焦点是事件驱动的：鼠标点框 / 打字设上，
    /// 点框外设下，回合起落交给 [`Self::set_turn_working`]。
    pub fn set_focused(&self, focused: bool) {
        self.state.lock().unwrap().unfocused = !focused;
    }

    pub fn focused(&self) -> bool {
        !self.state.lock().unwrap().unfocused
    }

    /// 每帧把「回合是否在跑」喂进来，只在边沿动焦点：
    ///
    /// - 起一轮且框是空的 → 失焦（Grok：`Build anything`、不露光标）
    /// - 收一轮 → 还回焦点，别让空框一直暗着
    ///
    /// 中间的帧一概不碰，鼠标点框、打字设的焦点才留得住。
    pub(crate) fn set_turn_working(&self, working: bool) {
        let mut state = self.state.lock().unwrap();
        if working == state.turn_working {
            return;
        }
        state.turn_working = working;
        if working {
            if state.input.trim().is_empty() && state.images.is_empty() {
                state.unfocused = true;
            }
        } else {
            state.unfocused = false;
        }
    }

    pub fn handle_paste(&self, text: &str) {
        let text = crate::app::clipboard::normalize_cr(text);
        if text.is_empty() {
            return;
        }
        let lines = text.lines().count();
        if lines >= PASTE_CHIP_LINES || text.len() > 4000 {
            let mut state = self.state.lock().unwrap();
            let input = state.input.clone();
            state.paste_bodies.retain(|(chip, _)| input.contains(chip));
            let n = lines.max(1);
            let chip = if text.len() > 4000 {
                format!("[Pasted: {}]", format_bytes(text.len()))
            } else {
                format!("[Pasted: {n} lines]")
            };
            insert_at_cursor(&mut state, &chip);
            state.paste_bodies.push((chip, text));
        } else {
            self.insert_str(&text);
        }
    }

    pub fn insert_image(&self, img: ClipboardImage) -> Result<PastedImage, String> {
        let mut state = self.state.lock().unwrap();
        if state.images.len() >= IMAGE_CAP {
            return Err(format!("最多 {IMAGE_CAP} 张图片"));
        }
        if img.width > 0 && img.height > 0 && (img.width < 8 || img.height < 8) {
            return Err(format!(
                "图片太小（{}×{}），至少 8×8",
                img.width, img.height
            ));
        }
        state.image_counter += 1;
        let pasted = PastedImage::from_clipboard(state.image_counter, img);
        let chip = format!("{} ", pasted.chip());
        insert_at_cursor(&mut state, &chip);
        state.images.push(pasted.clone());
        Ok(pasted)
    }

    pub fn preview_image(&self) -> Option<PastedImage> {
        let state = self.state.lock().unwrap();
        let cursor = state.cursor;
        for img in state.images.iter().rev() {
            let chip = img.chip();
            if let Some(at) = state.input.find(&chip) {
                if cursor >= at && cursor <= at + chip.len() + 1 {
                    return Some(img.clone());
                }
            }
        }
        state.images.last().cloned()
    }

    pub fn backspace(&self) {
        let mut state = self.state.lock().unwrap();
        // 有选区就是删这一段，别再往前退一个字符。
        if state.take_selection() {
            sync_chips(&mut state);
            return;
        }
        if state.cursor == 0 {
            return;
        }
        if let Some(range) = chip_touching(&state.input, state.cursor, true) {
            drop_chip_at(&mut state, range);
            return;
        }
        let cursor = state.cursor;
        let from = prev_boundary(&state.input, cursor);
        state.input.replace_range(from..cursor, "");
        state.cursor = from;
        state.slash_selected = None;
        state.file_dismissed = false;
        sync_chips(&mut state);
    }

    pub fn delete(&self) {
        let mut state = self.state.lock().unwrap();
        if state.take_selection() {
            sync_chips(&mut state);
            return;
        }
        if state.cursor >= state.input.len() {
            return;
        }
        if let Some(range) = chip_touching(&state.input, state.cursor, false) {
            drop_chip_at(&mut state, range);
            return;
        }
        let cursor = state.cursor;
        let to = next_boundary(&state.input, cursor);
        state.input.replace_range(cursor..to, "");
        state.slash_selected = None;
        state.file_dismissed = false;
        sync_chips(&mut state);
    }

    pub fn move_left(&self) {
        let mut state = self.state.lock().unwrap();
        let next = prev_boundary(&state.input, state.cursor);
        state.set_cursor(next);
    }

    pub fn move_right(&self) {
        let mut state = self.state.lock().unwrap();
        let next = next_boundary(&state.input, state.cursor);
        state.set_cursor(next);
    }

    pub fn move_home(&self) {
        let mut state = self.state.lock().unwrap();
        let start = line_start(&state.input, state.cursor);
        state.set_cursor(start);
    }

    pub fn move_end(&self) {
        let mut state = self.state.lock().unwrap();
        let end = line_end(&state.input, state.cursor);
        state.set_cursor(end);
    }

    pub fn move_buffer_start(&self) {
        let mut state = self.state.lock().unwrap();
        state.set_cursor(0);
    }

    pub fn move_buffer_end(&self) {
        let mut state = self.state.lock().unwrap();
        let end = state.input.len();
        state.set_cursor(end);
    }

    pub fn replace_range(&self, range: std::ops::Range<usize>, text: &str) {
        let mut state = self.state.lock().unwrap();
        let start = range.start.min(state.input.len());
        let end = range.end.min(state.input.len());
        state.input.replace_range(start..end, text);
        state.cursor = start + text.len();
        state.slash_selected = None;
        state.file_dismissed = false;
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.input.clear();
        state.cursor = 0;
        state.slash_selected = None;
        state.file_selected = 0;
        state.file_dismissed = false;
        state.last_at_query.clear();
        state.images.clear();
        state.image_counter = 0;
        state.paste_bodies.clear();
        state.last_sent = None;
    }

    pub fn take(&self) -> String {
        self.take_prompt().text
    }

    pub fn take_prompt(&self) -> TakenPrompt {
        let mut state = self.state.lock().unwrap();
        let mut text = std::mem::take(&mut state.input);
        for (chip, body) in state.paste_bodies.drain(..) {
            text = text.replace(&chip, &body);
        }
        if !text.trim().is_empty() {
            state.history.push(text.clone());
        }
        let images = std::mem::take(&mut state.images);
        state.history_idx = None;
        state.stash.clear();
        state.cursor = 0;
        state.slash_selected = None;
        state.file_selected = 0;
        state.file_dismissed = false;
        state.last_at_query.clear();
        state.image_counter = 0;
        TakenPrompt { text, images }
    }

    /// Remember the text actually submitted (after paste-chip expand / trim).
    pub fn note_sent(&self, text: &str) {
        self.state.lock().unwrap().last_sent = Some(text.to_string());
    }

    pub fn take_last_sent(&self) -> Option<String> {
        self.state.lock().unwrap().last_sent.take()
    }

    /// Put a cancelled in-flight prompt back in the composer (full text + images).
    pub fn restore_sent(&self, text: &str, images: Vec<PastedImage>) {
        let mut state = self.state.lock().unwrap();
        if state
            .history
            .last()
            .is_some_and(|h| h == text || h.trim() == text)
        {
            state.history.pop();
        }
        state.input = text.to_string();
        state.cursor = state.input.len();
        state.slash_selected = None;
        state.file_selected = 0;
        state.file_dismissed = false;
        state.last_at_query.clear();
        state.history_idx = None;
        state.last_sent = None;
        state.images = images;
        state.image_counter = state.images.iter().map(|img| img.n).max().unwrap_or(0);
        state.unfocused = false;
    }

    fn slash_settings(&self) -> Option<std::sync::Arc<AppSettings>> {
        self.ctx
            .as_ref()
            .and_then(|c| c.get::<AppSettings>(SETTINGS))
    }

    pub fn slash_snapshot(&self) -> crate::slash::SlashSnapshot {
        let extras = self.slash_extras();
        let settings = self.slash_settings();
        let state = self.state.lock().unwrap();
        crate::slash::snapshot_with_settings(
            &state.input,
            state.slash_selected.unwrap_or(0),
            &extras,
            settings.as_deref(),
        )
    }

    /// 用户是否亲手上下选过下拉里的某一行（而不是停在默认高亮上）。
    pub fn slash_picked(&self) -> bool {
        self.state.lock().unwrap().slash_selected.is_some()
    }

    pub fn slash_move(&self, delta: i16) {
        let extras = self.slash_extras();
        let settings = self.slash_settings();
        let mut state = self.state.lock().unwrap();
        let snap = crate::slash::snapshot_with_settings(
            &state.input,
            state.slash_selected.unwrap_or(0),
            &extras,
            settings.as_deref(),
        );
        if !snap.open || snap.matches.is_empty() {
            return;
        }
        let n = snap.matches.len() as i32;
        let cur = state.slash_selected.unwrap_or(0) as i32;
        state.slash_selected = Some((cur + delta as i32).rem_euclid(n) as usize);
    }

    pub fn apply_slash_insert(&self, display: &str) {
        self.set_text(display);
    }

    /// Grok `PromptWidget::set_text`: replace the composer, cursor at end.
    pub fn set_text(&self, text: &str) {
        let mut state = self.state.lock().unwrap();
        state.input = text.to_string();
        state.cursor = state.input.len();
        state.slash_selected = None;
        state.file_selected = 0;
        state.file_dismissed = false;
        if text.is_empty() {
            state.images.clear();
            state.image_counter = 0;
            state.paste_bodies.clear();
            state.last_at_query.clear();
        }
    }

    pub fn file_search_snapshot(&self) -> FileSearchSnapshot {
        let mut state = self.state.lock().unwrap();
        if let Some(ctx) = file_search::detect(&state.input, state.cursor) {
            if ctx.query != state.last_at_query {
                state.last_at_query = ctx.query;
                state.file_selected = 0;
            }
        } else {
            state.last_at_query.clear();
            state.file_dismissed = false;
        }
        file_search::snapshot(
            &state.input,
            state.cursor,
            state.file_selected,
            state.file_dismissed,
        )
    }

    pub fn file_search_move(&self, delta: i16) {
        let mut state = self.state.lock().unwrap();
        let snap = file_search::snapshot(
            &state.input,
            state.cursor,
            state.file_selected,
            state.file_dismissed,
        );
        if !snap.open || snap.matches.is_empty() {
            return;
        }
        let n = snap.matches.len() as i32;
        state.file_selected = (state.file_selected as i32 + delta as i32).rem_euclid(n) as usize;
    }

    pub fn file_search_dismiss(&self) {
        self.state.lock().unwrap().file_dismissed = true;
    }

    /// Insert the highlighted `@` path. Directories in dir-mode keep the
    /// trailing `/` so the popup stays open (Grok drill-down).
    pub fn accept_file_search(&self) -> bool {
        let snap = self.file_search_snapshot();
        let Some(hit) = snap.current().cloned() else {
            return false;
        };
        let text = self.text();
        let cursor = self.cursor();
        let Some(ctx) = file_search::detect(&text, cursor) else {
            return false;
        };
        let path = file_search::normalize_display_path(&hit.path).to_string();
        // 两个分支都是目录补尾斜杠，仅 is_dir 决定，合并为一条（行为不变）。
        let insert = if hit.is_dir {
            format!("{path}/")
        } else {
            format!("{path} ")
        };
        self.replace_range(ctx.path_range(), &insert);
        true
    }

    pub fn history(&self) -> Vec<String> {
        self.state.lock().unwrap().history.clone()
    }

    pub fn apply_text(&self, text: &str) {
        self.apply_slash_insert(text);
    }

    pub fn history_prev(&self) {
        let mut state = self.state.lock().unwrap();
        if state.history.is_empty() {
            return;
        }
        match state.history_idx {
            None => {
                state.stash = state.input.clone();
                let i = state.history.len() - 1;
                state.history_idx = Some(i);
                state.input = state.history[i].clone();
            }
            Some(0) => {}
            Some(i) => {
                let i = i - 1;
                state.history_idx = Some(i);
                state.input = state.history[i].clone();
            }
        }
        state.cursor = state.input.len();
        state.file_dismissed = false;
    }

    pub fn history_next(&self) {
        let mut state = self.state.lock().unwrap();
        let Some(i) = state.history_idx else {
            return;
        };
        if i + 1 >= state.history.len() {
            state.history_idx = None;
            state.input = std::mem::take(&mut state.stash);
        } else {
            state.history_idx = Some(i + 1);
            state.input = state.history[i + 1].clone();
        }
        state.cursor = state.input.len();
        state.file_dismissed = false;
    }

    /// Chrome (2) + content rows, capped at `max` (Grok: prompt ≤ half screen).
    pub fn desired_height(&self, width: u16, max: u16) -> u16 {
        let inner = width.saturating_sub(4).max(1) as usize;
        let text = self.text();
        let rows = visual_rows(&text, inner).len().max(1) as u16;
        (rows.saturating_add(2)).clamp(3, max.max(3))
    }

    /// 左键按下：命中输入框就记一个待定锚点并把光标挪过去，返回 `true`
    /// （调用点据此**不再**把这次按下交给滚动区）。
    pub fn mouse_down(&self, column: u16, row: u16) -> bool {
        let mut state = self.state.lock().unwrap();
        state.selection = None;
        state.pending = None;
        state.dragging = false;
        let area = state.last_area;
        let Some(byte) = byte_at(&state, area, column, row) else {
            return false;
        };
        // Clicking the box claims focus (border + caret); scrollback clicks blur
        // via the event loop so idle empty chrome can go dim.
        state.unfocused = false;
        state.pending = Some((column, row, byte));
        state.set_cursor_keep_selection(byte);
        true
    }

    /// 左键拖动：过了一格阈值才算选区——不然每次点击都会留下一个空选区闪一下。
    pub fn mouse_drag(&self, column: u16, row: u16) -> bool {
        let mut state = self.state.lock().unwrap();
        if let Some((col0, row0, anchor)) = state.pending {
            if column.abs_diff(col0) == 0 && row.abs_diff(row0) == 0 {
                return false;
            }
            state.pending = None;
            state.selection = Some((anchor, anchor));
            state.dragging = true;
        }
        if !state.dragging {
            return false;
        }
        let area = state.last_area;
        let Some(byte) = byte_at(&state, area, column, row) else {
            return true;
        };
        if let Some((_, head)) = state.selection.as_mut() {
            *head = byte;
        }
        state.set_cursor_keep_selection(byte);
        true
    }

    /// 左键抬起：拖出过非空选区就把它交出去（调用点负责写剪贴板）。
    /// 高亮留着，和滚动区一样等下一次按下再清。
    pub fn mouse_up(&self, column: u16, row: u16) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        state.pending = None;
        if !state.dragging {
            return None;
        }
        state.dragging = false;
        let area = state.last_area;
        if let Some(byte) = byte_at(&state, area, column, row) {
            if let Some((_, head)) = state.selection.as_mut() {
                *head = byte;
            }
        }
        let text = state.selected_text();
        if text.is_none() {
            state.selection = None;
        }
        text
    }

    /// 打字、回车、换会话……任何改动都让选区作废。
    /// 每帧开头清一次「我画在哪儿」。
    ///
    /// 输入框不是每帧都画（会话面板是独立全屏视图，权限/审批浮层也会顶掉它）。
    /// 不清的话 `last_area` 会留着上一帧的位置，鼠标点在盖住它的面板上会被判成
    /// 「点在输入框里」。清在**每帧入口**，而不是各个不画输入框的分支里各记
    /// 一次——后者漏一个就是这个 bug。
    pub fn begin_frame(&self) {
        self.state.lock().unwrap().last_area = Rect::ZERO;
    }

    /// 这个坐标是不是落在**这一帧真画出来的**输入框里。
    pub fn hit(&self, column: u16, row: u16) -> bool {
        let area = self.state.lock().unwrap().last_area;
        area.width > 0 && area.height > 0 && area.contains(Position { x: column, y: row })
    }

    pub fn clear_selection(&self) {
        let mut state = self.state.lock().unwrap();
        state.selection = None;
        state.pending = None;
        state.dragging = false;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.state.lock().unwrap().selected_text()
    }

    pub fn cursor_position(&self, area: Rect) -> Option<Position> {
        if area.height < 3 || area.width < 4 {
            return None;
        }
        let inner = area.width.saturating_sub(4).max(1) as usize;
        let state = self.state.lock().unwrap();
        if state.unfocused {
            return None;
        }
        let rows = visual_rows(&state.input, inner);
        let body_h = area.height.saturating_sub(2).max(1) as usize;
        let cursor_row = row_for_cursor(&rows, state.cursor);
        let start = (cursor_row + 1).saturating_sub(body_h);
        let vis = cursor_row.saturating_sub(start);
        let row = rows.get(cursor_row)?;
        let col_text =
            &state.input[row.byte_start..state.cursor.min(row.byte_end).max(row.byte_start)];
        let col =
            UnicodeWidthStr::width(row.prefix) as u16 + UnicodeWidthStr::width(col_text) as u16;
        Some(Position {
            x: area
                .x
                .saturating_add(2)
                .saturating_add(col.min(area.width.saturating_sub(4))),
            y: area.y.saturating_add(1).saturating_add(vis as u16),
        })
    }
}

impl Widget for &PromptWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 4 {
            return;
        }
        let theme = Theme::current();
        let bg = theme.bg_base;
        // 边框跟着焦点走：没焦点时用暗的那支。主题里两支颜色一直都在（注释写着
        // "dimmer prompt chrome" / "brighter when focused"），只是这里一直只取
        // 亮的那支，于是「这会儿打字到底进不进得去」看不出来。
        let border_color = if self.focused() {
            theme.prompt_border_active
        } else {
            theme.prompt_border
        };
        buf.set_style(area, Style::default().fg(theme.text_primary).bg(bg));

        let content = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content);

        let div_style = Style::default().fg(border_color).bg(bg);
        let left_x = area.x;
        let right_x = area.x + area.width.saturating_sub(1);
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, chunks[0].y)) {
                let ch = if x == left_x {
                    '\u{256d}'
                } else if x == right_x {
                    '\u{256e}'
                } else {
                    '\u{2500}'
                };
                cell.set_char(ch);
                cell.set_style(div_style);
            }
            if let Some(cell) = buf.cell_mut((x, chunks[2].y)) {
                let ch = if x == left_x {
                    '\u{2570}'
                } else if x == right_x {
                    '\u{256f}'
                } else {
                    '\u{2500}'
                };
                cell.set_char(ch);
                cell.set_style(div_style);
            }
        }

        let body = chunks[1];
        for y in body.y..body.y + body.height {
            if let Some(cell) = buf.cell_mut((left_x, y)) {
                cell.set_char('\u{2502}');
                cell.set_style(div_style);
            }
            if let Some(cell) = buf.cell_mut((right_x, y)) {
                cell.set_char('\u{2502}');
                cell.set_style(div_style);
            }
        }

        let mut state = self.state.lock().unwrap();
        // 鼠标坐标要按这一帧的位置换算，所以画完就记下来。
        state.last_area = area;
        let selection = state.selection_range();
        paint_info_line(
            buf,
            chunks[2],
            area,
            &state.chrome_info,
            bg,
            &theme,
            div_style,
        );

        let inner = area.width.saturating_sub(4).max(1) as usize;
        let rows = visual_rows(&state.input, inner);
        let style = Style::default().fg(theme.text_primary).bg(bg);
        let chip_style = Style::default().fg(theme.text_primary).bg(theme.paste_bg);
        let body_h = body.height as usize;
        let cursor_row = row_for_cursor(&rows, state.cursor);
        let start = (cursor_row + 1).saturating_sub(body_h.max(1));
        let text_x = area.x.saturating_add(2);
        for vis in 0..body_h {
            let idx = start + vis;
            let y = body.y + vis as u16;
            if y >= body.y + body.height {
                break;
            }
            let Some(row) = rows.get(idx) else {
                if vis == 0 && state.input.is_empty() {
                    buf.set_stringn(
                        text_x,
                        y,
                        "> ",
                        body.width.saturating_sub(2) as usize,
                        style,
                    );
                    if state.unfocused {
                        paint_placeholder(buf, text_x, y, body.width, bg, &theme);
                    }
                }
                break;
            };
            let slice = &state.input[row.byte_start..row.byte_end];
            paint_prompt_row(
                buf,
                text_x,
                y,
                body.width.saturating_sub(2),
                row.prefix,
                slice,
                style,
                chip_style,
            );
            // 选区**画在正文之上**：复用滚动区那支高亮，两处拖选看起来才是同
            // 一件事（`apply_selection_highlight` 还兼顾了 `Color::Reset` 主题）。
            if let Some(range) = selection.as_ref() {
                paint_row_selection(
                    buf,
                    text_x,
                    y,
                    body.width.saturating_sub(2),
                    row,
                    &state.input,
                    range,
                    &theme,
                );
            }
            if state.input.is_empty() && vis == 0 && state.unfocused {
                paint_placeholder(buf, text_x, y, body.width, bg, &theme);
            }
        }
    }
}

fn paint_placeholder(
    buf: &mut Buffer,
    text_x: u16,
    y: u16,
    body_width: u16,
    bg: ratatui::style::Color,
    theme: &Theme,
) {
    let ph_x = text_x.saturating_add(2);
    let avail = body_width.saturating_sub(4);
    if avail == 0 {
        return;
    }
    buf.set_stringn(
        ph_x,
        y,
        PLACEHOLDER,
        avail as usize,
        Style::default().fg(theme.gray).bg(bg),
    );
}

fn paint_info_line(
    buf: &mut Buffer,
    row: Rect,
    area: Rect,
    info: &str,
    bg: ratatui::style::Color,
    theme: &Theme,
    div_style: Style,
) {
    let info = info.trim();
    if info.is_empty() {
        return;
    }
    let max_w = area.width.saturating_sub(4);
    if max_w < 4 {
        return;
    }
    let label = format!(" {info} ");
    let trunc = truncate_width(&label, max_w as usize);
    let w = UnicodeWidthStr::width(trunc.as_str()) as u16;
    let right = area.x.saturating_add(area.width.saturating_sub(1));
    let x = right.saturating_sub(w).max(area.x.saturating_add(1));
    let fg = crate::grok::color::blend_color(bg, theme.text_secondary, 0.6).unwrap_or(theme.gray);
    let style = Style::default().fg(fg).bg(bg);
    buf.set_stringn(x, row.y, &trunc, w as usize, style);
    let _ = div_style;
}

/// 一帧里正文的布局：可见行、起始行号、正文左上角。渲染和命中判定**共用**它，
/// 否则两边各算一遍，窄窗折行一变就会错位。
struct BodyLayout {
    rows: Vec<VisualRow>,
    body: Rect,
    start: usize,
    text_x: u16,
    width: u16,
}

fn body_layout(state: &State, area: Rect) -> Option<BodyLayout> {
    if area.height < 3 || area.width < 4 {
        return None;
    }
    let body = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height.saturating_sub(2),
    };
    if body.height == 0 {
        return None;
    }
    let inner = area.width.saturating_sub(4).max(1) as usize;
    let rows = visual_rows(&state.input, inner);
    let cursor_row = row_for_cursor(&rows, state.cursor);
    let start = (cursor_row + 1).saturating_sub((body.height as usize).max(1));
    Some(BodyLayout {
        rows,
        body,
        start,
        text_x: area.x.saturating_add(2),
        width: body.width.saturating_sub(2),
    })
}

/// 屏幕坐标 → `input` 的字节下标。落在正文之外返回 `None`；落在某一行右边的
/// 空白处就吸到那一行末尾（拖选到行尾时的自然结果）。
fn byte_at(state: &State, area: Rect, column: u16, row: u16) -> Option<usize> {
    let layout = body_layout(state, area)?;
    if column < layout.text_x || column >= layout.text_x.saturating_add(layout.width) {
        // 左边框 / 右边框那两列不算正文，但纵向仍在框内时吸到最近的边。
        if !(area.x..area.x.saturating_add(area.width)).contains(&column) {
            return None;
        }
    }
    if row < layout.body.y || row >= layout.body.y.saturating_add(layout.body.height) {
        return None;
    }
    let vis = (row - layout.body.y) as usize;
    let idx = layout.start + vis;
    let Some(vrow) = layout.rows.get(idx) else {
        // 空行区：吸到全文末尾。
        return Some(state.input.len());
    };
    let slice = &state.input[vrow.byte_start..vrow.byte_end];
    let target = column.saturating_sub(layout.text_x);
    let mut col = UnicodeWidthStr::width(vrow.prefix) as u16;
    if target <= col {
        return Some(vrow.byte_start);
    }
    let mut byte = vrow.byte_start;
    for ch in slice.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        // 落在字符前半格算这个字符之前，后半格算之后——半格吸附，跟编辑器一致。
        if target < col.saturating_add(w.max(1)).saturating_sub(w / 2) {
            return Some(byte);
        }
        col = col.saturating_add(w);
        byte += ch.len_utf8();
    }
    Some(vrow.byte_end)
}

/// 把这一行落在选区里的那几列涂成高亮。
///
/// 按**显示列**算而不是按字节：CJK 一个字占两列，按字节涂会和文本错位半格。
#[allow(clippy::too_many_arguments)]
fn paint_row_selection(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    row: &VisualRow,
    input: &str,
    range: &Range<usize>,
    theme: &Theme,
) {
    if range.end <= row.byte_start || range.start >= row.byte_end {
        return;
    }
    let lo = range.start.max(row.byte_start);
    let hi = range.end.min(row.byte_end);
    let mut col = x.saturating_add(UnicodeWidthStr::width(row.prefix) as u16);
    let end_x = x.saturating_add(width);
    let mut byte = row.byte_start;
    for ch in input[row.byte_start..row.byte_end].chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if byte >= lo && byte < hi {
            for cx in col..col.saturating_add(w).min(end_x) {
                if let Some(cell) = buf.cell_mut((cx, y)) {
                    crate::scrollback::text_selection::apply_selection_highlight(theme, cell);
                }
            }
        }
        col = col.saturating_add(w);
        byte += ch.len_utf8();
        if col >= end_x {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn paint_prompt_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    prefix: &str,
    slice: &str,
    style: Style,
    chip_style: Style,
) {
    let line = format!("{prefix}{slice}");
    let chips = chip_ranges(&line);
    let mut col = x;
    let mut byte = 0usize;
    let end_x = x.saturating_add(width);
    for ch in line.chars() {
        if col >= end_x {
            break;
        }
        let next = byte + ch.len_utf8();
        let in_chip = chips.iter().any(|r| byte >= r.start && byte < r.end);
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if w == 0 {
            byte = next;
            continue;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch);
            cell.set_style(if in_chip { chip_style } else { style });
        }
        col = col.saturating_add(w);
        byte = next;
    }
}

pub fn paint_image_card(buf: &mut Buffer, area: Rect, image: &PastedImage, theme: &Theme) {
    if area.height < 5 || area.width < 16 {
        return;
    }
    let bg = theme.paste_bg;
    let fg = theme.text_primary;
    let dim = theme.gray;
    let border = Style::default().fg(theme.prompt_border_active).bg(bg);
    let title = format!(
        " Image #{} — {} · {}x{} · {} ",
        image.n,
        image.format_name(),
        image.width,
        image.height,
        image.size_label()
    );
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(Style::default().bg(bg));
            }
        }
    }
    let left = area.x;
    let right = area.x + area.width.saturating_sub(1);
    let top = area.y;
    let bot = area.y + area.height.saturating_sub(1);
    for x in left..=right {
        if let Some(cell) = buf.cell_mut((x, top)) {
            cell.set_char(if x == left {
                '\u{250c}'
            } else if x == right {
                '\u{2510}'
            } else {
                '\u{2500}'
            });
            cell.set_style(border);
        }
        if let Some(cell) = buf.cell_mut((x, bot)) {
            cell.set_char(if x == left {
                '\u{2514}'
            } else if x == right {
                '\u{2518}'
            } else {
                '\u{2500}'
            });
            cell.set_style(border);
        }
    }
    for y in top + 1..bot {
        if let Some(cell) = buf.cell_mut((left, y)) {
            cell.set_char('\u{2502}');
            cell.set_style(border);
        }
        if let Some(cell) = buf.cell_mut((right, y)) {
            cell.set_char('\u{2502}');
            cell.set_style(border);
        }
    }
    buf.set_stringn(
        left.saturating_add(1),
        top,
        &title,
        area.width.saturating_sub(2) as usize,
        Style::default().fg(fg).bg(bg),
    );
    let rows = [
        format!(" Format: {}", image.format_name()),
        format!(" Dimensions: {} x {}", image.width, image.height),
        format!(" Size: {}", image.size_label()),
    ];
    for (i, line) in rows.iter().enumerate() {
        let y = top + 1 + i as u16;
        if y >= bot {
            break;
        }
        buf.set_stringn(
            left.saturating_add(1),
            y,
            line,
            area.width.saturating_sub(2) as usize,
            Style::default().fg(dim).bg(bg),
        );
    }
}

fn insert_at_cursor(state: &mut State, s: &str) {
    // 选中之后打字是**替换**，不是插进选区中间。
    state.take_selection();
    let i = state.cursor.min(state.input.len());
    state.input.insert_str(i, s);
    state.cursor = i + s.len();
    state.slash_selected = None;
    state.file_dismissed = false;
    // Typing into an idle-blurred empty box claims focus again.
    state.unfocused = false;
}

fn chip_ranges(s: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = s[i..].find(']') {
                let token = &s[i..i + end + 1];
                if token.starts_with("[Image #") || token.starts_with("[Pasted:") {
                    out.push(i..i + end + 1);
                    i += end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn chip_touching(input: &str, cursor: usize, deleting_left: bool) -> Option<Range<usize>> {
    for range in chip_ranges(input) {
        if deleting_left {
            if cursor == range.end || (cursor > range.start && cursor <= range.end) {
                return Some(range);
            }
        } else if cursor >= range.start && cursor < range.end {
            return Some(range);
        }
    }
    None
}

fn drop_chip_at(state: &mut State, range: Range<usize>) {
    let chip = state.input.get(range.clone()).unwrap_or("").to_string();
    state.input.replace_range(range.clone(), "");
    state.cursor = range.start;
    state.slash_selected = None;
    state.file_dismissed = false;
    state.images.retain(|img| img.chip() != chip);
    state.paste_bodies.retain(|(c, _)| c != &chip);
}

fn sync_chips(state: &mut State) {
    let input = state.input.clone();
    state.images.retain(|img| input.contains(&img.chip()));
    state.paste_bodies.retain(|(chip, _)| input.contains(chip));
}

fn format_bytes(n: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if n < 1024 {
        format!("{n} B")
    } else if (n as f64) < MB {
        format!("{:.1} KB", n as f64 / KB)
    } else {
        format!("{:.1} MB", n as f64 / MB)
    }
}

fn truncate_width(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

struct VisualRow {
    prefix: &'static str,
    byte_start: usize,
    byte_end: usize,
}

fn visual_rows(text: &str, content_w: usize) -> Vec<VisualRow> {
    let wrap_w = content_w.saturating_sub(2).max(1);
    let mut rows = Vec::new();
    let mut offset = 0usize;
    let parts: Vec<&str> = text.split('\n').collect();
    for (li, line) in parts.iter().enumerate() {
        let prefix = if li == 0 { "> " } else { "  " };
        for (start, end) in wrap_byte_ranges(line, wrap_w) {
            rows.push(VisualRow {
                prefix,
                byte_start: offset + start,
                byte_end: offset + end,
            });
        }
        offset += line.len();
        if li + 1 < parts.len() {
            offset += 1;
        }
    }
    if rows.is_empty() {
        rows.push(VisualRow {
            prefix: "> ",
            byte_start: 0,
            byte_end: 0,
        });
    }
    rows
}

fn wrap_byte_ranges(s: &str, width: usize) -> Vec<(usize, usize)> {
    if s.is_empty() {
        return vec![(0, 0)];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut w = 0usize;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w > 0 && w + cw > width {
            out.push((start, i));
            start = i;
            w = cw;
        } else {
            w += cw;
        }
    }
    out.push((start, s.len()));
    out
}

fn row_for_cursor(rows: &[VisualRow], cursor: usize) -> usize {
    rows.iter()
        .rposition(|r| r.byte_start <= cursor)
        .unwrap_or(0)
}

fn prev_boundary(s: &str, i: usize) -> usize {
    if i == 0 {
        return 0;
    }
    let mut i = i - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    i + s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(0)
}

fn line_start(s: &str, cursor: usize) -> usize {
    s[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

fn line_end(s: &str, cursor: usize) -> usize {
    s[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_at_cursor_not_only_at_end() {
        let prompt = PromptWidget::default();
        prompt.insert_str("ac");
        prompt.move_left();
        prompt.push('b');
        assert_eq!(prompt.text(), "abc");
        assert_eq!(prompt.cursor(), 2);
    }

    #[test]
    fn backspace_deletes_before_cursor() {
        let prompt = PromptWidget::default();
        prompt.insert_str("ab");
        prompt.backspace();
        assert_eq!(prompt.text(), "a");
        assert_eq!(prompt.cursor(), 1);
    }

    #[test]
    fn paste_short_text_is_inline() {
        let prompt = PromptWidget::default();
        prompt.handle_paste("hello");
        assert_eq!(prompt.text(), "hello");
    }

    #[test]
    fn paste_four_lines_becomes_chip() {
        let prompt = PromptWidget::default();
        prompt.handle_paste("a\nb\nc\nd");
        assert_eq!(prompt.text(), "[Pasted: 4 lines]");
        let taken = prompt.take_prompt();
        assert_eq!(taken.text, "a\nb\nc\nd");
    }

    #[test]
    fn image_chip_and_backspace_drops_it() {
        let prompt = PromptWidget::default();
        let img = ClipboardImage::from_bytes(
            {
                let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
                png.extend_from_slice(&[0, 0, 0, 13]);
                png.extend_from_slice(b"IHDR");
                png.extend_from_slice(&32u32.to_be_bytes());
                png.extend_from_slice(&32u32.to_be_bytes());
                png.extend_from_slice(&[8, 2, 0, 0, 0]);
                png
            },
            Some("image/png"),
        )
        .unwrap();
        prompt.insert_image(img).unwrap();
        assert!(prompt.text().starts_with("[Image #1]"));
        prompt.backspace();
        prompt.backspace();
        assert!(prompt.text().is_empty());
    }

    fn render_at(prompt: &PromptWidget, area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        prompt.render(area, &mut buf);
        buf
    }

    /// 输入框里要能用鼠标框选、抬手即复制。
    ///
    /// 全屏应用把终端的原生拖选顶掉了（alt-screen + 鼠标上报），所以这一段必须
    /// 自己做，否则用户**根本没法把自己刚打的字复制出去**。滚动区早就这么干了。
    #[test]
    fn dragging_across_the_box_selects_and_hands_back_the_text() {
        let prompt = PromptWidget::default();
        prompt.insert_str("hello world");
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area); // 记下 last_area，命中判定要用

        // 正文从 x=2 起，`> ` 前缀占两列，所以第一个字符在 x=4。
        assert!(prompt.mouse_down(4, 1), "按在框里要被输入框接住");
        assert!(prompt.mouse_drag(9, 1));
        assert_eq!(prompt.mouse_up(9, 1).as_deref(), Some("hello"));
        // 高亮留着，和滚动区一样等下一次按下再清。
        assert_eq!(prompt.selection_text().as_deref(), Some("hello"));
    }

    /// 选中之后按删除，删的是**整段选中**，不是再往前退一个字符。
    ///
    /// 这是最容易漏的一环：选区做出来了、也画出来了，但编辑动作不认它，用户
    /// 选中一句话按删除，只掉了一个字。
    #[test]
    fn deleting_with_a_selection_removes_the_whole_selection() {
        let area = Rect::new(0, 0, 40, 3);
        for (label, act) in [("backspace", 0usize), ("delete", 1)] {
            let prompt = PromptWidget::default();
            prompt.insert_str("hello world");
            let _ = render_at(&prompt, area);
            assert!(prompt.mouse_down(4, 1));
            assert!(prompt.mouse_drag(9, 1));
            assert_eq!(prompt.mouse_up(9, 1).as_deref(), Some("hello"), "{label}");

            if act == 0 {
                prompt.backspace();
            } else {
                prompt.delete();
            }
            assert_eq!(prompt.text(), " world", "{label}");
            assert_eq!(prompt.cursor(), 0, "{label}");
            assert!(prompt.selection_text().is_none(), "{label}");
        }
    }

    /// 选中之后打字是**替换**，不是插进选区中间。
    #[test]
    fn typing_over_a_selection_replaces_it() {
        let prompt = PromptWidget::default();
        prompt.insert_str("hello world");
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area);
        assert!(prompt.mouse_down(4, 1));
        assert!(prompt.mouse_drag(9, 1));
        assert_eq!(prompt.mouse_up(9, 1).as_deref(), Some("hello"));

        prompt.push('h');
        prompt.push('i');
        assert_eq!(prompt.text(), "hi world");
        assert!(prompt.selection_text().is_none());
    }

    /// 挪光标才是「作废」——选区不该跟着方向键一起走。
    #[test]
    fn moving_the_cursor_drops_the_selection() {
        let prompt = PromptWidget::default();
        prompt.insert_str("hello world");
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area);
        assert!(prompt.mouse_down(4, 1));
        assert!(prompt.mouse_drag(9, 1));
        assert!(prompt.mouse_up(9, 1).is_some());

        // 抬手时光标停在选区末尾（"hello" 之后），右移一格跨过空格。
        prompt.move_right();
        assert!(prompt.selection_text().is_none());
        prompt.backspace();
        assert_eq!(
            prompt.text(),
            "helloworld",
            "作废之后就是普通退格，只掉一个字符"
        );
    }

    /// 框外的按下不归输入框管——否则点滚动区会把选区起在输入框里。
    #[test]
    fn a_press_outside_the_box_is_not_the_prompts() {
        let prompt = PromptWidget::default();
        prompt.insert_str("abc");
        let area = Rect::new(0, 10, 40, 3);
        let _ = render_at(&prompt, area);
        assert!(!prompt.mouse_down(4, 2), "框上面那行不是输入框");
        assert!(!prompt.mouse_down(4, 20), "框下面那行也不是");
        assert!(prompt.mouse_down(4, 11));
    }

    /// 没拖动就只是点一下：移光标，不留选区。
    #[test]
    fn a_plain_click_moves_the_cursor_without_selecting() {
        let prompt = PromptWidget::default();
        prompt.insert_str("hello");
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area);
        assert!(prompt.mouse_down(6, 1));
        assert!(prompt.mouse_up(6, 1).is_none());
        assert!(prompt.selection_text().is_none());
    }

    /// CJK 一个字占两列：按显示列换算，不能按字节，否则选出来的和看到的差半格。
    #[test]
    fn selection_counts_display_columns_not_bytes() {
        let prompt = PromptWidget::default();
        prompt.insert_str("试测试");
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area);
        assert!(prompt.mouse_down(4, 1));
        assert!(prompt.mouse_drag(8, 1));
        assert_eq!(prompt.mouse_up(8, 1).as_deref(), Some("试测"));
    }

    /// 没焦点时边框用暗的那支：不然「这会儿打字进不进得去」看不出来。
    #[test]
    fn the_border_follows_focus() {
        let theme = Theme::current();
        let prompt = PromptWidget::default();
        let area = Rect::new(0, 0, 20, 3);

        prompt.set_focused(true);
        let bright = render_at(&prompt, area);
        prompt.set_focused(false);
        let dim = render_at(&prompt, area);

        let corner = |buf: &Buffer| buf[(0, 0)].style().fg;
        assert_eq!(corner(&bright), Some(theme.prompt_border_active));
        assert_eq!(corner(&dim), Some(theme.prompt_border));
        assert_ne!(theme.prompt_border, theme.prompt_border_active);
    }

    #[test]
    fn unfocused_empty_composer_hides_cursor() {
        let prompt = PromptWidget::default();
        prompt.set_focused(false);
        let area = Rect::new(0, 0, 80, 5);
        assert!(prompt.cursor_position(area).is_none());
        prompt.set_focused(true);
        assert!(prompt.cursor_position(area).is_some());
    }

    /// 点在框里要把焦点要回来：边框亮、光标露出来。
    #[test]
    fn mouse_down_on_the_box_claims_focus() {
        let prompt = PromptWidget::default();
        prompt.set_focused(false);
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area);
        assert!(!prompt.focused());
        assert!(prompt.mouse_down(4, 1));
        assert!(prompt.focused());
    }

    /// 打字也要把失焦的空框唤回来，不然边框一直暗着。
    #[test]
    fn typing_reclaims_focus_on_an_idle_blurred_box() {
        let prompt = PromptWidget::default();
        prompt.set_focused(false);
        assert!(!prompt.focused());
        prompt.push('a');
        assert!(prompt.focused());
        assert_eq!(prompt.text(), "a");
    }

    /// `frame.rs` 每帧喂 `working` 的那两行，测里照着跑，别只测组件方法。
    fn frame(prompt: &PromptWidget, working: bool) {
        prompt.set_turn_working(working);
        if prompt.can_send() {
            prompt.set_focused(true);
        }
    }

    /// 回合跑着的时候点输入框要能聚焦。之前每帧 `set_focused(can_send())` 会把
    /// `mouse_down` 刚设上的焦点当场抹掉，点了跟没点一样。
    #[test]
    fn clicking_the_composer_mid_turn_keeps_focus() {
        let prompt = PromptWidget::default();
        let area = Rect::new(0, 0, 40, 3);
        let _ = render_at(&prompt, area);

        frame(&prompt, true); // 起一轮：空框失焦
        assert!(!prompt.focused());

        assert!(prompt.mouse_down(4, 1));
        assert!(prompt.focused(), "回合中点框应聚焦");

        frame(&prompt, true); // 后续帧不许再抢
        frame(&prompt, true);
        assert!(prompt.focused(), "回合中的后续帧不该抹掉点击拿到的焦点");
    }

    /// 回合结束后空框要亮回来：清空输入不动 `unfocused`，空闲帧又不再强制聚焦，
    /// 边框会一直暗着直到用户点一下。
    #[test]
    fn empty_composer_refocuses_when_the_turn_ends() {
        let prompt = PromptWidget::default();

        prompt.insert_str("hi");
        frame(&prompt, false);
        assert!(prompt.focused());

        let _ = prompt.take(); // 发送：输入清空
        frame(&prompt, true);
        assert!(!prompt.focused(), "回合中空框应失焦");

        frame(&prompt, false); // 回合结束
        assert!(prompt.focused(), "回合结束后空框应恢复聚焦");
        frame(&prompt, false);
        assert!(prompt.focused());
    }

    /// 回合中打字，焦点也要留住（`insert_at_cursor` 设的焦点 + 有内容）。
    #[test]
    fn typing_mid_turn_keeps_focus() {
        let prompt = PromptWidget::default();
        frame(&prompt, true);
        assert!(!prompt.focused());

        prompt.push('x');
        frame(&prompt, true);
        assert!(prompt.focused(), "回合中打字应保持聚焦");
    }

    /// 起一轮时框里已经有下一条消息：不能把用户正在编辑的内容弄失焦。
    #[test]
    fn a_turn_starting_does_not_blur_a_non_empty_composer() {
        let prompt = PromptWidget::default();
        prompt.insert_str("next");
        frame(&prompt, true);
        assert!(prompt.focused());
    }

    #[test]
    fn restore_sent_puts_back_full_prompt_and_pops_history() {
        let prompt = PromptWidget::default();
        prompt.insert_str("undo me [Image #1]");
        let taken = prompt.take_prompt();
        assert!(prompt.text().is_empty());
        assert_eq!(
            prompt.history().last().map(String::as_str),
            Some("undo me [Image #1]")
        );
        prompt.restore_sent(&taken.text, taken.images);
        assert_eq!(prompt.text(), "undo me [Image #1]");
        assert!(prompt.history().is_empty());
    }

    #[test]
    fn chrome_info_sits_on_the_right_of_the_bottom_border() {
        use crate::theme::Theme;
        use ratatui::style::Color;

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let theme = Theme::current();
        let area = Rect::new(0, 0, 40, 3);
        let row = Rect::new(0, 0, 40, 1);
        paint_info_line(
            &mut buf,
            row,
            area,
            "守望 · 询问",
            Color::Black,
            &theme,
            Style::default(),
        );
        let first_content = (0..40).find(|&x| {
            let s = buf[(x, 0)].symbol();
            !s.is_empty() && s != " "
        });
        assert!(
            first_content.is_some_and(|x| x > 15),
            "expected right-aligned chrome, first glyph at {first_content:?}"
        );
        let joined: String = (0..40).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(joined.contains('守') && joined.contains('问'), "{joined:?}");
    }
}

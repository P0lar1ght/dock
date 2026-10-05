//! Named `"tui.overlays"`：终端浮层的注册表。
//!
//! 新浮层不用再往 `Overlay` enum 加变体、往事件循环的 `match` 里加分支：实现
//! [`OverlayView`]（怎么画、怎么响应按键），挂一颗插件登记进来，用
//! `Overlay::view(kind, arg)` 打开。事件循环只认 `Overlay::View`，按 `kind` 查表。
//! 老浮层碰到再迁（插件面板 `slot` 是第一个）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::grok::picker::PickerHits;

pub const TUI_OVERLAYS: &str = "tui.overlays";

/// 一个已打开的注册浮层的状态：打开时带的参数（如插槽 id）+ 滚动位置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewState {
    pub arg: String,
    pub scroll: usize,
}

/// 交给浮层的输入。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayInput {
    Esc,
    Enter,
    /// 滚轮 / ↑↓：负数往上。
    Scroll(i16),
    Char(char),
}

/// 浮层对输入的回应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayReply {
    Keep,
    Close,
}

/// 一种浮层。只拿 ctx 与自己的状态，画什么、按键干什么都在这里。
pub trait OverlayView: Send + Sync {
    fn paint(&self, ctx: &Context, buf: &mut Buffer, area: Rect, state: &ViewState) -> PickerHits;
    fn input(&self, ctx: &Context, state: &mut ViewState, input: OverlayInput) -> OverlayReply;
}

/// Named `"tui.overlays"`。调用点 live-lookup。
#[derive(Clone, Default)]
pub struct OverlayViews {
    views: Arc<Mutex<HashMap<String, Arc<dyn OverlayView>>>>,
}

impl OverlayViews {
    /// 登记一种浮层。同名已有就失败。
    pub fn register(&self, kind: &str, view: Arc<dyn OverlayView>) -> cordis::Result<Disposable> {
        let kind = kind.to_string();
        {
            let mut views = self.views.lock().unwrap();
            if views.contains_key(&kind) {
                return Err(cordis::Error::plugin(format!("浮层 {kind} 已登记")));
            }
            views.insert(kind.clone(), view);
        }
        let views = self.views.clone();
        Ok(Disposable::from_fn(move || {
            views.lock().unwrap().remove(&kind);
        }))
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn OverlayView>> {
        self.views.lock().unwrap().get(kind).cloned()
    }
}

/// 挂 `"tui.overlays"` 表。各种浮层由各自的插件登记（见 [`crate::views::slot_overlay`]）。
pub fn overlays() -> Plugin {
    plugin("tui.overlays", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TUI_OVERLAYS, OverlayViews::default())?))
    })
}

/// 功能插件登记自己的浮层：随插件 fiber 注销。
pub fn register_overlay(
    ctx: &Context,
    kind: &str,
    view: Arc<dyn OverlayView>,
) -> cordis::Result<()> {
    let table = ctx.require::<OverlayViews>(TUI_OVERLAYS)?;
    let d = table.register(kind, view)?;
    ctx.effect("tui.overlays.register", |scope| {
        scope.own(d);
        Ok(())
    })?;
    Ok(())
}

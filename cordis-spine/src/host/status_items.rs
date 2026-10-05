//! Named `"status.items"`：插件放在界面上的常驻小状态（`docs/PLUGIN-VIEWS.md` 状态项）。
//!
//! 一项 = 短文字 + 色调，可带一句说明和「点它打开哪个插件面板」。登记的插件卸下时
//! 跟着消失；内容随时可改（`update`）。增删改都发 [`STATUS_CHANGED`]（载荷 id），
//! 终端底栏和网关（推给 GUI）据此重画。
//!
//! 旧的插槽 `hud: true` 也算状态项：[`StatusItems::list`] 把它们按面板标题 + 正文
//! 第一行转进来，`surface` 指向那个面板。

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_base::view::Tone;
use indexmap::IndexMap;

use crate::host::tui_slots::TuiSlots;
use crate::names::{STATUS_CHANGED, STATUS_ITEMS, TUI_SLOTS};

/// 一个状态项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusItem {
    pub id: String,
    /// 尽量短：一个词或一个数。
    pub text: String,
    pub tone: Tone,
    pub tooltip: Option<String>,
    /// 点它打开哪个插件面板（`"tui.slots"` 的 id）。
    pub surface: Option<String>,
}

impl StatusItem {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            tone: Tone::Default,
            tooltip: None,
            surface: None,
        }
    }
}

/// Named `"status.items"`。调用点 live-lookup。
#[derive(Clone, Default)]
pub struct StatusItems {
    items: Arc<Mutex<IndexMap<String, StatusItem>>>,
    ctx: Option<Context>,
}

impl StatusItems {
    fn on(ctx: Context) -> Self {
        Self {
            ctx: Some(ctx),
            ..Self::default()
        }
    }

    fn changed(&self, id: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.emit(STATUS_CHANGED, id.to_string());
        }
    }

    /// 登记一项。同 id 已有就失败；返回的 `Disposable` 注销它。
    pub fn register(&self, item: StatusItem) -> cordis::Result<Disposable> {
        let id = item.id.trim().to_string();
        if id.is_empty() {
            return Err(cordis::Error::plugin("状态项 id 不能为空"));
        }
        {
            let mut items = self.items.lock().unwrap();
            if items.contains_key(&id) {
                return Err(cordis::Error::plugin(format!("状态项 {id} 已登记")));
            }
            items.insert(
                id.clone(),
                StatusItem {
                    id: id.clone(),
                    ..item
                },
            );
        }
        self.changed(&id);
        let this = self.clone();
        Ok(Disposable::from_fn(move || {
            this.items.lock().unwrap().shift_remove(&id);
            this.changed(&id);
        }))
    }

    /// 改一项的内容（id 不变）。没登记过回 `false`。
    pub fn update(&self, item: StatusItem) -> bool {
        let id = item.id.clone();
        let updated = {
            let mut items = self.items.lock().unwrap();
            match items.get_mut(&id) {
                Some(slot) if *slot != item => {
                    *slot = item;
                    true
                }
                Some(_) => return true,
                None => false,
            }
        };
        if updated {
            self.changed(&id);
        }
        updated
    }

    /// 去掉一项（插件主动清掉）。之后登记它的那颗插件卸下时再注销一次也无妨。
    pub fn remove(&self, id: &str) -> bool {
        let removed = self.items.lock().unwrap().shift_remove(id).is_some();
        if removed {
            self.changed(id);
        }
        removed
    }

    /// 全部状态项：登记的，加上 `hud: true` 的插槽（`surface` 指向它自己）。
    pub fn list(&self) -> Vec<StatusItem> {
        let mut out: Vec<StatusItem> = self.items.lock().unwrap().values().cloned().collect();
        let slots = self
            .ctx
            .as_ref()
            .and_then(|ctx| ctx.get::<TuiSlots>(TUI_SLOTS));
        if let Some(slots) = slots {
            for (id, line) in slots.hud_lines() {
                if out.iter().any(|i| i.id == id) {
                    continue;
                }
                out.push(StatusItem {
                    surface: Some(id.clone()),
                    tooltip: slots.title(&id),
                    ..StatusItem::new(id, line)
                });
            }
        }
        out
    }
}

pub fn status_items() -> Plugin {
    plugin("status-items", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(STATUS_ITEMS, StatusItems::on(ctx.clone()))?,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::tui_slots::{tui_slots, SlotHandler, SlotKeyResult};

    struct Hud;
    impl SlotHandler for Hud {
        fn title(&self) -> String {
            "便签".into()
        }
        fn hud(&self) -> bool {
            true
        }
        fn render(&self) -> String {
            "3 条\n第二行".into()
        }
        fn on_key(&self, _key: &str) -> SlotKeyResult {
            SlotKeyResult::Keep
        }
    }

    /// 登记、改、注销都发 `status/changed`；`hud` 插槽按第一行转成状态项、点它开那个面板。
    #[tokio::test]
    async fn items_change_and_hud_slots_join_the_list() {
        let root = Context::new();
        for p in [tui_slots(), status_items()] {
            root.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let _l = root
            .on(STATUS_CHANGED, move |id: &String| {
                log.lock().unwrap().push(id.clone())
            })
            .unwrap();
        let status = root.get::<StatusItems>(STATUS_ITEMS).unwrap();
        let d = status
            .register(StatusItem {
                tone: Tone::Accent,
                ..StatusItem::new("deploy", "部署中 60%")
            })
            .unwrap();
        assert!(status.register(StatusItem::new("deploy", "x")).is_err());
        assert!(status.update(StatusItem {
            tone: Tone::Success,
            ..StatusItem::new("deploy", "已部署")
        }));
        assert!(
            status.update(StatusItem {
                tone: Tone::Success,
                ..StatusItem::new("deploy", "已部署")
            }),
            "内容没变也算成功"
        );
        assert!(!status.update(StatusItem::new("nope", "x")));

        let slots = root.get::<TuiSlots>(TUI_SLOTS).unwrap();
        let _hud = slots.register("memo".into(), Arc::new(Hud)).unwrap();
        let list = status.list();
        assert_eq!(list[0].text, "已部署");
        assert_eq!(list[0].tone, Tone::Success);
        assert_eq!(list[1].id, "memo");
        assert_eq!(list[1].text, "3 条");
        assert_eq!(list[1].surface.as_deref(), Some("memo"));

        d.dispose_sync();
        assert_eq!(status.list().len(), 1);
        assert_eq!(*seen.lock().unwrap(), vec!["deploy", "deploy", "deploy"]);
    }
}

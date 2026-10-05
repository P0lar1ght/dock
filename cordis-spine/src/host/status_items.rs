//! Named `"status.items"`：插件放在界面上的常驻小状态（`docs/PLUGIN-VIEWS.md` 状态项）。
//!
//! 一项 = 短文字 + 色调，可带一句说明和「点它打开哪个插件面板」。登记的插件卸下时
//! 跟着消失；内容随时可改（`update`）。增删改都发 [`STATUS_CHANGED`]（载荷 id），
//! 终端底栏和网关（推给 GUI）据此重画。
//!
//! 旧的插槽 `hud: true` 也算状态项：[`StatusItems::list`] 把它们按面板标题 + 正文
//! 第一行转进来，`surface` 指向那个面板。

use std::sync::atomic::{AtomicU64, Ordering};
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

/// 登记凭据：谁登记的谁才能改、才能删。卸下旧登记不会误删后来别人同 id 的那一项。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StatusToken(u64);

/// Named `"status.items"`。调用点 live-lookup。
#[derive(Clone, Default)]
pub struct StatusItems {
    items: Arc<Mutex<IndexMap<String, (StatusToken, StatusItem)>>>,
    next: Arc<AtomicU64>,
    ctx: Option<Context>,
}

/// id 去掉首尾空白、不能为空；文字不能为空（空的「● 」没有意义）。
fn checked(item: StatusItem) -> cordis::Result<StatusItem> {
    let id = item.id.trim().to_string();
    if id.is_empty() {
        return Err(cordis::Error::plugin("状态项 id 不能为空"));
    }
    if item.text.trim().is_empty() {
        return Err(cordis::Error::plugin(format!("状态项 {id} 的文字不能为空")));
    }
    Ok(StatusItem { id, ..item })
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

    /// 登记一项。同 id 已有（不管是谁的）就失败。凭据用来改 / 删它；`Disposable`
    /// 只注销**这一次**登记——之后别人同 id 再登记的那一项不受影响。
    pub fn register(&self, item: StatusItem) -> cordis::Result<(StatusToken, Disposable)> {
        let item = checked(item)?;
        let id = item.id.clone();
        let token = StatusToken(self.next.fetch_add(1, Ordering::Relaxed));
        {
            let mut items = self.items.lock().unwrap();
            if items.contains_key(&id) {
                return Err(cordis::Error::plugin(format!("状态项 {id} 已被登记")));
            }
            items.insert(id.clone(), (token, item));
        }
        self.changed(&id);
        let this = self.clone();
        Ok((
            token,
            Disposable::from_fn(move || {
                this.remove(token, &id);
            }),
        ))
    }

    /// 用凭据改一项的内容（id 不变）。不是这个凭据登记的、或已经没了回 `false`。
    pub fn update(&self, token: StatusToken, item: StatusItem) -> bool {
        let Ok(item) = checked(item) else {
            return false;
        };
        let id = item.id.clone();
        let changed = {
            let mut items = self.items.lock().unwrap();
            match items.get_mut(&id) {
                Some((owner, current)) if *owner == token => {
                    let changed = *current != item;
                    *current = item;
                    changed
                }
                _ => return false,
            }
        };
        if changed {
            self.changed(&id);
        }
        true
    }

    /// 用凭据去掉一项。不是这个凭据登记的就不动。
    pub fn remove(&self, token: StatusToken, id: &str) -> bool {
        let id = id.trim();
        let removed = {
            let mut items = self.items.lock().unwrap();
            match items.get(id) {
                Some((owner, _)) if *owner == token => items.shift_remove(id).is_some(),
                _ => false,
            }
        };
        if removed {
            self.changed(id);
        }
        removed
    }

    /// 全部状态项：登记的，加上 `hud: true` 的插槽（`surface` 指向它自己）。
    pub fn list(&self) -> Vec<StatusItem> {
        let mut out: Vec<StatusItem> = self
            .items
            .lock()
            .unwrap()
            .values()
            .map(|(_, item)| item.clone())
            .collect();
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
        let (token, d) = status
            .register(StatusItem {
                tone: Tone::Accent,
                ..StatusItem::new(" deploy ", "部署中 60%")
            })
            .unwrap();
        assert!(status.register(StatusItem::new("deploy", "x")).is_err());
        assert!(status.register(StatusItem::new("blank", "  ")).is_err());
        assert!(status.update(
            token,
            StatusItem {
                tone: Tone::Success,
                ..StatusItem::new("deploy", "已部署")
            }
        ));
        assert!(
            status.update(
                token,
                StatusItem {
                    tone: Tone::Success,
                    ..StatusItem::new(" deploy", "已部署")
                }
            ),
            "内容没变也算成功（id 照样去空白）"
        );
        assert!(!status.update(token, StatusItem::new("nope", "x")));

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

    /// 归属：别人拿不到凭据就改不了、删不了；旧登记卸下不会误删后来同 id 的那一项。
    #[tokio::test]
    async fn items_belong_to_whoever_registered_them() {
        let root = Context::new();
        root.plugin(status_items(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let status = root.get::<StatusItems>(STATUS_ITEMS).unwrap();
        let (a, a_dispose) = status.register(StatusItem::new("x", "A 的")).unwrap();
        let (b_other, _keep) = status.register(StatusItem::new("y", "B 的")).unwrap();
        assert!(!status.update(b_other, StatusItem::new("x", "B 改的")));
        assert!(!status.remove(b_other, "x"));
        assert!(status.remove(a, "x"));
        let (b, _b_keep) = status.register(StatusItem::new("x", "B 的")).unwrap();
        a_dispose.dispose_sync();
        let x = status
            .list()
            .into_iter()
            .find(|i| i.id == "x")
            .expect("B 的还在");
        assert_eq!(x.text, "B 的");
        assert!(status.update(b, StatusItem::new("x", "B 改的")));
    }
}

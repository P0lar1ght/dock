//! Named `"tool.views"`：插件给某个工具登记的卡片视图（`docs/PLUGIN-VIEWS.md` 工具卡）。
//!
//! 渲染函数拿这一次调用的参数、输出和成败，回一棵视图树（或 `None` = 用通用卡片）。
//! 终端展开工具卡时调用；网关在 GUI 展开时经 `tool/view` 按需调用。视图只给人看，
//! 不进模型历史。增删发 [`TOOL_VIEWS_CHANGED`]（载荷工具名），终端据 [`ToolViews::revision`]
//! 让排版缓存失效。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_base::view::ViewNode;
use indexmap::IndexMap;

use crate::names::{TOOL_VIEWS, TOOL_VIEWS_CHANGED};

/// 一次工具调用，交给渲染函数。
#[derive(Clone, Copy, Debug)]
pub struct ToolViewInput<'a> {
    pub name: &'a str,
    /// 原样的参数 JSON 字符串。
    pub arguments: &'a str,
    pub output: &'a str,
    pub failed: bool,
}

/// 渲染函数。回 `None` 表示这次不给视图（用通用卡片）。
pub type ToolViewFn = Arc<dyn Fn(&ToolViewInput<'_>) -> Option<ViewNode> + Send + Sync>;

/// Named `"tool.views"`。调用点 live-lookup。
#[derive(Clone, Default)]
pub struct ToolViews {
    views: Arc<Mutex<IndexMap<String, ToolViewFn>>>,
    revision: Arc<AtomicU64>,
    ctx: Option<Context>,
}

impl ToolViews {
    fn on(ctx: Context) -> Self {
        Self {
            ctx: Some(ctx),
            ..Self::default()
        }
    }

    fn changed(&self, name: &str) {
        self.revision.fetch_add(1, Ordering::Relaxed);
        if let Some(ctx) = &self.ctx {
            ctx.emit(TOOL_VIEWS_CHANGED, name.to_string());
        }
    }

    /// 给工具 `name` 登记渲染函数。同名已登记就失败。
    pub fn register(&self, name: &str, view: ToolViewFn) -> cordis::Result<Disposable> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(cordis::Error::plugin("工具名不能为空"));
        }
        {
            let mut views = self.views.lock().unwrap();
            if views.contains_key(&name) {
                return Err(cordis::Error::plugin(format!("工具 {name} 已有卡片视图")));
            }
            views.insert(name.clone(), view);
        }
        self.changed(&name);
        let this = self.clone();
        Ok(Disposable::from_fn(move || {
            this.views.lock().unwrap().shift_remove(&name);
            this.changed(&name);
        }))
    }

    pub fn has(&self, name: &str) -> bool {
        self.views.lock().unwrap().contains_key(name)
    }

    /// 有视图的工具名。
    pub fn names(&self) -> Vec<String> {
        self.views.lock().unwrap().keys().cloned().collect()
    }

    /// 画这一次调用；没登记或渲染函数回 `None` 都是 `None`。
    pub fn render(&self, input: &ToolViewInput<'_>) -> Option<ViewNode> {
        // 先取出函数再调用：渲染函数可能跑插件脚本，不能拿着锁。
        let view = self.views.lock().unwrap().get(input.name).cloned()?;
        view(input)
    }

    /// 每次增删加一。排版缓存把它算进 key，登记 / 卸下视图后重画。
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
}

pub fn tool_views() -> Plugin {
    plugin("tool-views", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TOOL_VIEWS, ToolViews::on(ctx.clone()))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn renders_registered_tools_and_tracks_changes() {
        let root = Context::new();
        root.plugin(tool_views(), ()).unwrap().wait().await.unwrap();
        let views = root.get::<ToolViews>(TOOL_VIEWS).unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let _l = root
            .on(TOOL_VIEWS_CHANGED, move |n: &String| {
                log.lock().unwrap().push(n.clone())
            })
            .unwrap();
        let d = views
            .register(
                "deploy_status",
                Arc::new(|input: &ToolViewInput<'_>| {
                    (!input.failed)
                        .then(|| ViewNode::parse(&json!({ "type": "text", "text": input.output })))
                }),
            )
            .unwrap();
        assert!(views
            .register("deploy_status", Arc::new(|_: &ToolViewInput<'_>| None))
            .is_err());
        let input = |failed| ToolViewInput {
            name: "deploy_status",
            arguments: "{}",
            output: "运行中",
            failed,
        };
        assert_eq!(views.render(&input(false)).unwrap().to_plain(), "运行中");
        assert!(views.render(&input(true)).is_none(), "渲染函数可以不给视图");
        let rev = views.revision();
        d.dispose_sync();
        assert!(views.revision() > rev);
        assert!(views.render(&input(false)).is_none());
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["deploy_status", "deploy_status"]
        );
    }
}

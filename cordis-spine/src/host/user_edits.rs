//! 用户在文件面板里直接改了工作区文件（GUI 经网关 `fs/write|mkdir|rename|trash`）：
//! 按页记下来，这一页下一步采样前用一条 system-reminder 告诉 agent——免得它拿之前读到
//! 的旧内容覆盖用户的修改。
//!
//! 在跑的一轮在下一个步骤边界就能看到；空闲的页等用户发下一条消息。只告诉这一页
//! 自己（子代理、别的页身份不同，取不到）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::types::{StepStart, ORDER_STEP_START_USER_EDITS};

use crate::names::{SESSIONS, STEP_START, USER_EDITS};
use crate::session::log::Sessions;

/// 一页最多攒这么多条，再多只计数（提醒别太长）。
const MAX_NOTES: usize = 30;

/// 各页还没告诉 agent 的用户改动（键是页的会话身份）。
#[derive(Clone, Default)]
pub struct UserEdits {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

#[derive(Default)]
struct Pending {
    notes: Vec<String>,
    dropped: usize,
}

impl UserEdits {
    /// 记一条（「编辑 src/a.ts」）。同一条已经在等就不重复记。
    pub fn record(&self, identity: &str, note: String) {
        let mut map = self.pending.lock().unwrap();
        let page = map.entry(identity.to_string()).or_default();
        if page.notes.contains(&note) {
            return;
        }
        if page.notes.len() >= MAX_NOTES {
            page.dropped += 1;
        } else {
            page.notes.push(note);
        }
    }

    /// 取走这一页攒的，排成一条提醒；没有就是 `None`。
    pub fn take_reminder(&self, identity: &str) -> Option<String> {
        let page = self.pending.lock().unwrap().remove(identity)?;
        let mut list = page
            .notes
            .iter()
            .map(|n| format!("- {n}"))
            .collect::<Vec<_>>();
        if page.dropped > 0 {
            list.push(format!("- ……另有 {} 处", page.dropped));
        }
        Some(format!(
            "<system-reminder>\n用户刚在文件面板里直接改了工作区文件（不是你改的）：\n{}\n\
             你之前读到的这些文件内容可能已经过时：要用到时先重新读，别拿旧内容覆盖用户的修改。\
             不用为此单独回复。\n</system-reminder>",
            list.join("\n")
        ))
    }
}

/// 记一条用户改动到 `page` 这一页（网关在写盘成功后调）。没挂插件就什么也不做。
pub fn record_user_edit(page: &Context, note: impl Into<String>) {
    let (Some(edits), Some(sessions)) = (
        page.get::<UserEdits>(USER_EDITS),
        page.get::<Sessions>(SESSIONS),
    ) else {
        return;
    };
    edits.record(sessions.identity(), note.into());
}

pub fn user_edits() -> Plugin {
    plugin("user-edits", Inject::new(), |ctx, _: &()| {
        let edits = UserEdits::default();
        ctx.provide(USER_EDITS, edits.clone())?;
        let handle = ctx.on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            if let Some(body) = edits.take_reminder(&next.identity) {
                next.remind(ORDER_STEP_START_USER_EDITS, body);
            }
            next
        })?;
        Ok(Some(handle))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_is_per_page_deduped_and_taken_once() {
        let edits = UserEdits::default();
        edits.record("main", "编辑 a.ts".into());
        edits.record("main", "编辑 a.ts".into());
        edits.record("main#2", "新建 b.md".into());
        let body = edits.take_reminder("main").unwrap();
        assert!(body.starts_with("<system-reminder>"), "{body}");
        assert_eq!(body.matches("编辑 a.ts").count(), 1, "{body}");
        assert!(!body.contains("b.md"), "别的页的不混进来");
        assert!(edits.take_reminder("main").is_none(), "取过就清");
        assert!(edits.take_reminder("main#2").is_some());
    }

    #[test]
    fn long_lists_are_capped() {
        let edits = UserEdits::default();
        for i in 0..MAX_NOTES + 5 {
            edits.record("main", format!("编辑 f{i}.ts"));
        }
        let body = edits.take_reminder("main").unwrap();
        assert!(body.contains("另有 5 处"), "{body}");
    }

    #[tokio::test]
    async fn step_start_injects_once_for_the_recording_page() {
        let ctx = Context::new();
        ctx.plugin(user_edits(), ()).unwrap().wait().await.unwrap();
        let edits = ctx.get::<UserEdits>(USER_EDITS).unwrap();
        edits.record("main", "移到废纸篓 old/".into());
        let run = |id: &str| {
            let start = StepStart::new(0, id);
            let seed = start.clone();
            ctx.waterfall(STEP_START, start, move || seed)
                .into_reminders()
        };
        assert!(run("child-1").is_empty(), "子代理身份不同，取不到");
        let got = run("main");
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("移到废纸篓 old/"));
        assert!(run("main").is_empty());
    }
}

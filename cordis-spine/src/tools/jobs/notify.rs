//! 后台任务跑完了，主动告诉起它的那一页（同 Claude Code 的后台命令完成通知）。
//!
//! 以前模型只能拿 `job` 加长 `timeout_ms` 干等，或者隔一会儿查一次：等的那几分钟这一轮
//! 挂着，用户插话也进不来，等到点还常常没跑完。现在后台任务跑完时，通知进那一页的信箱
//! （和子代理回报同一个信箱）：
//!
//! - 那一页在跑：下一步开始、采样之前送到（`drain_parent_mailbox`，不夹在工具调用和
//!   结果中间）。
//! - 那一页闲着：叫醒它开一轮（会话 actor 的信箱轮），不用用户再发话。
//!
//! 不叫醒的情况：
//! - 模型已经拿到结果了（`job` 读到它跑完、自己 `kill_task` 停的）：根本不发；已经排进
//!   信箱、还没送到的撤掉。
//! - 被别人停掉的（任务面板）：送，但不叫醒，等那一页下一次采样带上。
//! - 子代理起的：送进子代理自己的信箱，在它下一步带上；不叫醒（子代理只由父级叫醒）。
//!
//! 一秒内跑完的几条攒成一次叫醒：一条一叫，几条前后脚跑完的任务会让那一页连开几轮。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordis::{plugin, Context, Inject, Plugin};

use super::{JobEnd, JobFinished, JobReported, Jobs};
use crate::names::{JOBS, JOB_FINISHED, JOB_REPORTED, SUBAGENTS};
use crate::tools::task::Subagents;

/// 一页的第一条跑完之后，再等这么久把同一页的一起送。
pub(crate) const BATCH_WINDOW: Duration = Duration::from_secs(1);

/// 每一页攒着、还没送进信箱的跑完通知。
type Pending = Arc<Mutex<HashMap<String, Vec<JobFinished>>>>;

pub fn job_notify() -> Plugin {
    plugin("job-notify", Inject::new(), |ctx, _: &()| {
        let pending: Pending = Arc::default();
        let finished = {
            let root = ctx.clone();
            let pending = pending.clone();
            ctx.on(JOB_FINISHED, move |done: &JobFinished| {
                hold(&root, &pending, done.clone());
            })?
        };
        let reported = {
            let root = ctx.clone();
            ctx.on(JOB_REPORTED, move |r: &JobReported| {
                if let Some(sub) = root.get::<Subagents>(SUBAGENTS) {
                    sub.retract_job_notice(&r.id);
                }
            })?
        };
        ctx.effect("job-notify", |scope| {
            scope.own(finished);
            scope.own(reported);
            Ok(())
        })?;
        Ok(None)
    })
}

/// 攒进这一页的批次；是这一页这批的第一条就定一个 [`BATCH_WINDOW`] 之后的送出。
fn hold(root: &Context, pending: &Pending, done: JobFinished) {
    // 不是在哪一页的一轮里起的：没有哪个模型在等它。
    let Some(page) = done.owner.clone() else {
        return;
    };
    let first = {
        let mut pending = pending.lock().unwrap();
        let batch = pending.entry(page.clone()).or_default();
        batch.push(done);
        batch.len() == 1
    };
    if !first {
        return;
    }
    let root = root.clone();
    let pending = pending.clone();
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(async move {
                tokio::time::sleep(BATCH_WINDOW).await;
                deliver(&root, &pending, &page);
            });
        }
        // 任务都跑在 tokio 上，走不到这里；真走到了就别攒，直接送。
        Err(_) => deliver(&root, &pending, &page),
    }
}

/// 把 `page` 攒着的这批送进它的信箱。送之前再看一眼：攒着的这一秒里模型可能已经
/// 用 `job` 读到了结果。
fn deliver(root: &Context, pending: &Pending, page: &str) {
    let batch = pending.lock().unwrap().remove(page).unwrap_or_default();
    let Some(sub) = root.get::<Subagents>(SUBAGENTS) else {
        return;
    };
    let jobs = root.get::<Jobs>(JOBS);
    for done in batch {
        if jobs.as_ref().is_some_and(|j| j.is_reported(&done.id)) {
            continue;
        }
        let wakes = cordis_base::types::is_main_identity(page) && done.end != JobEnd::Killed;
        sub.notify_job_done(page, &done.id, render_notice(&done), wakes);
    }
}

/// 通知正文：怎么结束的、跑了多久、命令、输出结尾，以及去哪看完整输出。
pub(crate) fn render_notice(done: &JobFinished) -> String {
    let kind = if done.is_monitor {
        "后台 monitor"
    } else {
        "后台任务"
    };
    let id = &done.id;
    let how = match &done.end {
        JobEnd::Exited(0) => "结束了：exit 0".to_string(),
        JobEnd::Exited(code) => format!("结束了：exit {code}（失败）"),
        JobEnd::Signaled => "被信号终止了".to_string(),
        JobEnd::Killed => "被停掉了（不是你用 kill_task 停的）".to_string(),
        JobEnd::SpawnFailed => "没能启动".to_string(),
    };
    let mut text = format!(
        "{kind} {id} {how}，跑了 {}。这是后台任务的完成通知，不是用户发的消息。\n命令：{}",
        human_elapsed(done.elapsed),
        done.command.trim()
    );
    if let Some(desc) = done.description.as_deref().filter(|d| !d.trim().is_empty()) {
        text.push_str(&format!("（{}）", desc.trim()));
    }
    let ending = done.ending.trim_end();
    if done.ending_is_all {
        if ending.is_empty() || ending == "(no output)" {
            text.push_str("\n没有输出。");
        } else {
            text.push_str(&format!("\n输出（共 {} 行）：\n{ending}", done.lines));
        }
        return text;
    }
    let shown = ending.lines().count();
    text.push_str(&format!(
        "\n输出结尾（共 {} 行，下面是最后 {shown} 行）：\n{ending}",
        done.lines
    ));
    match &done.spill {
        Some(path) => text.push_str(&format!(
            "\n完整输出在 {path}，用 read_file 读（要看前面就从 offset=1 起）。"
        )),
        None => text.push_str(&format!("\n完整输出用 job 工具读：job_ids=[\"{id}\"]。")),
    }
    text
}

/// `3m12s` / `42s` / `0.4s`。
fn human_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else if secs >= 10 {
        format!("{secs}s")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(end: JobEnd, ending: &str, all: bool) -> JobFinished {
        JobFinished {
            id: "job-7".into(),
            owner: Some("main".into()),
            command: "cargo test -p cordis-spine".into(),
            description: Some("Run spine tests".into()),
            is_monitor: false,
            end,
            elapsed: Duration::from_secs(192),
            ending: ending.into(),
            ending_is_all: all,
            lines: 3021,
            spill: None,
        }
    }

    /// 通知要说清四件事：哪条、怎么结束的、跑了多久、结尾输出；还要说明它不是用户的话，
    /// 免得模型把它当成用户在催。
    #[test]
    fn the_notice_says_how_it_ended_and_shows_the_ending() {
        let text = render_notice(&finished(
            JobEnd::Exited(101),
            "test result: FAILED. 3 passed; 1 failed\n",
            false,
        ));
        assert!(
            text.starts_with("后台任务 job-7 结束了：exit 101（失败）"),
            "{text}"
        );
        assert!(text.contains("跑了 3m12s"), "{text}");
        assert!(text.contains("不是用户发的消息"), "{text}");
        assert!(
            text.contains("cargo test -p cordis-spine（Run spine tests）"),
            "{text}"
        );
        assert!(text.contains("共 3021 行，下面是最后 1 行"), "{text}");
        assert!(text.contains("test result: FAILED"), "{text}");
        assert!(
            text.contains("job_ids=[\"job-7\"]"),
            "没落盘要指向 job 工具：{text}"
        );
    }

    /// 输出落了盘：指向文件，不指向 `job`（`job` 只给开头 2KB 预览，看不到全文）。
    #[test]
    fn a_spilled_output_points_at_the_file() {
        let mut done = finished(JobEnd::Exited(0), "ok\n", false);
        done.spill = Some("/tmp/dock/tool-output/job-7.txt".into());
        let text = render_notice(&done);
        assert!(text.contains("exit 0，跑了"), "{text}");
        assert!(text.contains("/tmp/dock/tool-output/job-7.txt"), "{text}");
        assert!(!text.contains("job_ids"), "{text}");
    }

    /// 输出就这么点：整段给出，不提「完整输出」。
    #[test]
    fn a_short_output_is_shown_whole() {
        let mut done = finished(JobEnd::Exited(0), "built in 3s\n", true);
        done.lines = 1;
        let text = render_notice(&done);
        assert!(text.contains("输出（共 1 行）：\nbuilt in 3s"), "{text}");
        assert!(!text.contains("完整输出"), "{text}");

        let quiet = render_notice(&finished(JobEnd::Exited(0), "(no output)", true));
        assert!(quiet.contains("没有输出。"), "{quiet}");
    }
}

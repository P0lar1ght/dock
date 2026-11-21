//! `dock schedule …`：从命令行（脚本、git hook、本机 CI、快捷指令）看 / 跑 / 暂停定时任务。
//!
//! 直接读写 `$DOCK_HOME/schedules.json`（同一把 `flock`），不经网关：持有任务的 Dock
//! 一秒内照办；现在没有 Dock 在运行时，`run` 记下来，下次打开 Dock 接管时马上跑。
//! 任务 id 可以只写开头几位，只要不重。

use std::io::Write;

use cordis_spine::{Cron, CronJob, CronPatch, FireReason, RunOutcome};

pub enum ScheduleCmd {
    List,
    Run { id: String, input: Option<String> },
    Pause(String),
    Resume(String),
}

pub const SCHEDULE_USAGE: &str =
    "用法：dock schedule list | run <id> [--input <文本>] | pause <id> | resume <id>";

/// 解析 `dock schedule` 后面的参数。
pub fn parse_schedule_args(args: &[String]) -> Result<ScheduleCmd, String> {
    let mut it = args.iter();
    let action = it.next().map(String::as_str);
    let cmd = match action {
        Some("list") | Some("ls") | None => ScheduleCmd::List,
        Some("run") => {
            let id = it.next().ok_or("dock schedule run 需要任务 id")?.clone();
            let mut input = None;
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--input" | "-i" => {
                        input = Some(it.next().ok_or("--input 后面要跟文本")?.clone());
                    }
                    other => return Err(format!("dock schedule run 不认识参数 {other}")),
                }
            }
            return Ok(ScheduleCmd::Run { id, input });
        }
        Some("pause") => {
            ScheduleCmd::Pause(it.next().ok_or("dock schedule pause 需要任务 id")?.clone())
        }
        Some("resume") => {
            ScheduleCmd::Resume(it.next().ok_or("dock schedule resume 需要任务 id")?.clone())
        }
        Some(other) => return Err(format!("dock schedule 不认识 {other}\n{SCHEDULE_USAGE}")),
    };
    if let Some(extra) = it.next() {
        return Err(format!("dock schedule 不认识参数 {extra}"));
    }
    Ok(cmd)
}

/// 照 `cmd` 办，结果写到 `out`。
pub fn run_schedule_cmd(cron: &Cron, cmd: ScheduleCmd, out: &mut impl Write) -> Result<(), String> {
    let line = |out: &mut dyn Write, text: String| {
        let _ = writeln!(out, "{text}");
    };
    match cmd {
        ScheduleCmd::List => {
            let jobs = cron.list();
            if jobs.is_empty() {
                line(out, "没有定时任务。".into());
            }
            for job in &jobs {
                line(out, describe(job));
            }
            if jobs.iter().any(|j| !j.holder_live && !j.finished()) {
                line(
                    out,
                    "（标着「没有 Dock 在跑」的任务要等下次打开 Dock 才会运行）".into(),
                );
            }
        }
        ScheduleCmd::Run { id, input } => {
            let job = resolve(cron, &id)?;
            let context = input.map(|text| format!("Input passed to this run:\n{text}"));
            cron.trigger(&job.id, None, FireReason::Manual, context)
                .map_err(|e| e.to_string())?;
            let when = if job.holder_live {
                "Dock 一秒内送出"
            } else {
                "现在没有 Dock 在运行，下次打开 Dock 时马上跑"
            };
            line(out, format!("已请求运行 {}：{when}", job.id));
        }
        ScheduleCmd::Pause(id) => set_paused(cron, &id, true, out)?,
        ScheduleCmd::Resume(id) => set_paused(cron, &id, false, out)?,
    }
    Ok(())
}

fn set_paused(cron: &Cron, id: &str, paused: bool, out: &mut impl Write) -> Result<(), String> {
    let job = resolve(cron, id)?;
    cron.patch(
        &job.id,
        None,
        CronPatch {
            paused: Some(paused),
            ..CronPatch::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let verb = if paused { "已暂停" } else { "已继续" };
    let _ = writeln!(out, "{verb} {}", job.id);
    Ok(())
}

/// 完整 id，或不重的开头几位。
fn resolve(cron: &Cron, needle: &str) -> Result<CronJob, String> {
    let jobs = cron.list();
    if let Some(job) = jobs.iter().find(|j| j.id == needle) {
        return Ok(job.clone());
    }
    let hits: Vec<&CronJob> = jobs.iter().filter(|j| j.id.starts_with(needle)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!(
            "没有定时任务 {needle}（dock schedule list 看全部）"
        )),
        many => Err(format!(
            "{needle} 对上了 {} 个任务，多写几位：{}",
            many.len(),
            many.iter()
                .map(|j| j.id.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )),
    }
}

/// 一行：id、触发方式、状态、上次运行、提问开头。
fn describe(job: &CronJob) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let state = if job.paused {
        "已暂停".to_string()
    } else if job.finished() {
        "已跑完".to_string()
    } else if job.pending {
        "马上要跑".to_string()
    } else {
        match job.next_at_ms {
            Some(at) => format!("下次 {}", local_time(at, now)),
            None => "等着触发".to_string(),
        }
    };
    let last = job
        .last_run()
        .map(|run| {
            let how = match run.outcome {
                RunOutcome::Sent => "送达",
                RunOutcome::Coalesced => "跳过（上一次还没跑完）",
                RunOutcome::Failed => "没送到",
            };
            format!(" · 上次 {} {how}", local_time(run.at_ms, now))
        })
        .unwrap_or_default();
    let live = if job.holder_live || job.finished() {
        ""
    } else {
        " · 没有 Dock 在跑"
    };
    let prompt: String = job.prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let prompt = if prompt.chars().count() > 48 {
        format!("{}…", prompt.chars().take(47).collect::<String>())
    } else {
        prompt
    };
    format!(
        "{}  {}  {state}{last}{live}\n    {prompt}",
        job.id,
        job.schedule.label()
    )
}

/// 相对现在：「3 分钟后」「2 小时前」。
fn local_time(ms: u64, now: u64) -> String {
    let (gap, suffix) = if ms >= now {
        (ms - now, "后")
    } else {
        (now - ms, "前")
    };
    let mins = gap / 60_000;
    let span = if mins < 1 {
        "不到 1 分钟".to_string()
    } else if mins < 60 {
        format!("{mins} 分钟")
    } else if mins < 48 * 60 {
        format!("{} 小时 {} 分钟", mins / 60, mins % 60)
    } else {
        format!("{} 天", mins / (24 * 60))
    };
    format!("{span}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::{CronSchedule, NewCronTask};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_subcommands() {
        assert!(matches!(parse_schedule_args(&[]), Ok(ScheduleCmd::List)));
        assert!(matches!(
            parse_schedule_args(&args(&["run", "cron-1", "--input", "发布 v1.2"])),
            Ok(ScheduleCmd::Run { id, input: Some(input) }) if id == "cron-1" && input == "发布 v1.2"
        ));
        assert!(matches!(
            parse_schedule_args(&args(&["pause", "cron-1"])),
            Ok(ScheduleCmd::Pause(id)) if id == "cron-1"
        ));
        assert!(parse_schedule_args(&args(&["run"])).is_err());
        assert!(parse_schedule_args(&args(&["frobnicate"])).is_err());
        assert!(parse_schedule_args(&args(&["list", "extra"])).is_err());
    }

    /// `run` 记一次手动运行（带上附的话），持有它的 Dock 下一轮送出；id 写开头几位也认。
    #[test]
    fn run_requests_a_manual_fire_and_pause_toggles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(cordis_spine::SCHEDULES_FILE);
        let cron = Cron::at(&path);
        let id = cron
            .create(NewCronTask::new(CronSchedule::Manual, "发版前跑一遍检查"))
            .unwrap();

        // 另一个进程里的 CLI：自己开一份落在同一个文件上的。
        let cli = Cron::at(&path);
        let mut out = Vec::new();
        run_schedule_cmd(
            &cli,
            ScheduleCmd::Run {
                id: id[..id.len() - 1].to_string(),
                input: Some("v1.2".into()),
            },
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(&format!("已请求运行 {id}")), "{text}");

        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].reason, FireReason::Manual);
        assert!(tick.fires[0]
            .context
            .as_deref()
            .is_some_and(|c| c.contains("v1.2")));

        let mut out = Vec::new();
        run_schedule_cmd(&cli, ScheduleCmd::Pause(id.clone()), &mut out).unwrap();
        assert!(cron.get(&id).unwrap().paused);
        let mut out = Vec::new();
        run_schedule_cmd(&cli, ScheduleCmd::List, &mut out).unwrap();
        let listed = String::from_utf8(out).unwrap();
        assert!(listed.contains("已暂停"), "{listed}");
        assert!(listed.contains("发版前跑一遍检查"), "{listed}");
        assert!(
            run_schedule_cmd(&cli, ScheduleCmd::Resume("nope".into()), &mut Vec::new()).is_err()
        );
    }
}

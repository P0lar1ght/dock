//! `bash` workspace tool (alias `run_terminal_cmd`).

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::tools::fs_common::{bool_field, int_field, parse_args, resolve, str_field};
use crate::tools::jobs::Jobs;
use cordis_base::types::ToolSpec;

const BASH_PARAMS: &str = r#"{"type":"object","properties":{"command":{"type":"string","description":"The bash command to run."},"description":{"type":"string","description":"Clear, concise description of what this command does in active voice, 5-10 words (shown to the user in the permission prompt and the UI). Examples: \"git status\" -> \"Show working tree status\"; \"npm install\" -> \"Install package dependencies\"."},"workdir":{"type":"string","description":"Working directory for this command. Defaults to the session cwd; if you pass a relative workdir it resolves against the session cwd. Prefer this over a leading cd. IMPORTANT: relative paths inside command then resolve against workdir, not against the session cwd — with workdir \"sub\", write \"src/file.txt\", not \"sub/src/file.txt\"."},"timeout_ms":{"type":"integer","description":"How long to wait in the foreground, in ms. Defaults to 300000 (5 min) and is capped at it. On expiry the command is NOT killed: it moves to the background and you get a job id plus whatever it printed so far."},"is_background":{"type":"boolean","description":"Set to true for long-running commands (dev servers, long builds). Returns a job id immediately; collect with the job tool, stop with kill_task."},"block_until_ms":{"type":"integer","description":"Foreground wait in ms. 0 backgrounds immediately."}},"required":["command"]}"#;

const BASH_DESC: &str = "Run a bash command in the workspace and return its output.\n\
- Do not use bash for file work: `cat`/`head`/`sed -n` → read_file, `grep`/`rg` → grep, `find` → glob, `ls` → list_dir, `sed -i` → search_replace. The dedicated tools are cheaper, are not gated behind a permission prompt, keep working in plan mode, and report what they truncated. Shell `grep` also reads everything on disk including build output (`target/`), so a repo-wide search the grep tool finishes in tens of milliseconds can take bash minutes.\n\
- Each call runs in a fresh shell: cwd, variables and functions do not persist between calls. Pass workdir instead of using `cd`. Once you pass workdir, every relative path in the command is relative to it — do not also prefix those paths with the directory you just moved into.\n\
- A foreground command that outlives timeout_ms (default and cap 300000 ms) is not killed: it moves to the background and you get a job id plus the output so far. Never re-run it — collect with the job tool.\n\
- Set is_background true (or block_until_ms: 0) for dev servers and long builds: you get a job id immediately and check it with the job tool, stop it with kill_task.\n\
- Piping command output into `grep` (e.g. `cargo test 2>&1 | grep FAILED`) is what bash is for.\n\
- Output is capped; the head and tail are kept and the middle is reported as elided.";

/// 前台 bash 的阻塞预算。到点把命令**转入后台**并带回已产出的输出
/// （见 [`detach_to_background`]），不是杀掉。
///
/// Grok 这里是 30s（`block_until_ms` 的省略默认值），用意是催模型把长命令交后台。
/// 在本仓库太短：一条 `cargo clippy -p cordis-spine` 就 1 分多钟，`cargo test` 全量
/// 更久，30s 到点必然被收掉——模型只能反复「起后台 + `job` 轮询」，多花
/// 的回合比省下的等待贵。放宽到 5 分钟：仍有上限（这一轮挂不死）。覆盖用的 env
/// 对齐 Grok 的 `GROK_MAX_FOREGROUND_BLOCK_MS`。
pub(crate) const FOREGROUND_MS_ENV: &str = "DOCK_BASH_FOREGROUND_MS";
const DEFAULT_FOREGROUND_MS: u64 = 300_000;

fn foreground_budget() -> Duration {
    std::env::var(FOREGROUND_MS_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(DEFAULT_FOREGROUND_MS))
}

/// 本次调用的前台预算：模型传的 `timeout_ms` 优先，但**只能收紧不能放宽**。
///
/// 放宽会让一条命令把整轮挂住；收紧是有用的——模型知道 `cargo check` 该 30s
/// 内回来，超了就是卡住了，早点拿到部分输出比干等五分钟强。
fn call_budget(v: &Value) -> Duration {
    let ceiling = foreground_budget();
    match int_field(v, "timeout_ms").filter(|ms| *ms > 0) {
        Some(ms) => Duration::from_millis(ms as u64).min(ceiling),
        None => ceiling,
    }
}

/// 解析 `workdir`：相对路径按 cwd 展开；必须是已存在的目录。
///
/// 不存在就直接报错，而不是让 bash 在一个意外的目录里把命令跑掉——后者会产生
/// 「命令看起来成功了但作用在错误的地方」这种最难查的失败。
fn resolve_workdir(v: &Value) -> Result<Option<PathBuf>, String> {
    let Some(raw) = str_field(v, &["workdir", "cwd"]) else {
        return Ok(None);
    };
    let path = resolve(&raw);
    if !path.is_dir() {
        return Err(format!(
            "Error: workdir {} 不是一个已存在的目录",
            path.display()
        ));
    }
    Ok(Some(path))
}

pub(crate) async fn run(
    args: &str,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    jobs: Option<&Jobs>,
) -> String {
    let v = parse_args(args);
    let Some(command) = str_field(&v, &["command"]) else {
        return "Error: command is required".into();
    };
    let description = str_field(&v, &["description"]);
    let workdir = match resolve_workdir(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let background = bool_field(&v, "is_background")
        || bool_field(&v, "background")
        || int_field(&v, "block_until_ms") == Some(0);
    if background {
        if let Some(jobs) = jobs {
            let id = jobs.start_ex_in(command, description, false, workdir);
            // Grok `format_default_prompt` backgrounded + `background_retrieval_hint`.
            return format!(
                "[Command moved to background]\n\n\
                 job_id: {id}\n\n\
                 The command is still running in the background. You can continue with other tasks.\n\
                 Use the job tool with job_ids=[\"{id}\"] when you need the output."
            );
        }
    }
    // 前台也走 `Jobs`：同一套并发抽干 + 增量累积 + 输出上限，且 TUI 能在命令
    // 还在跑的时候就读到它的输出（tasks pane / scrollback 都读 JobSnapshot）。
    // 没挂 `"jobs"` 服务时（单测、精简装配）临时起一张本地表，行为完全一致，
    // 只是没人能查它 —— 所以后台请求仍然退回前台执行，不发无处可查的 job_id。
    let local;
    // 有没有**别人能查**的任务表，决定了超时能不能转后台：本地表随本次调用一起
    // 析构，往外发它的 job_id 等于发一张空头支票。
    let collectable = jobs.is_some();
    let jobs = match jobs {
        Some(jobs) => jobs,
        None => {
            local = Jobs::new();
            &local
        }
    };
    let id = jobs.start_foreground_ex(&command, description, workdir);
    let budget = call_budget(&v);
    let start = std::time::Instant::now();
    loop {
        // 轮询只读完成位；输出只在真正要返回时取一次，避免每 20ms 白拼一个
        // 最大 20KB 的 String。
        match jobs.is_done(&id) {
            Some(true) => {
                let out = jobs.snapshot(&id).map(|s| s.output).unwrap_or_default();
                jobs.forget(&id);
                return out;
            }
            // 任务凭空消失：只有我们自己会 forget，正常不会走到。
            None => return "(no output)".into(),
            Some(false) => {}
        }
        if is_cancelled() {
            return finish_early(jobs, &id, "cancelled".into()).await;
        }
        if start.elapsed() >= budget {
            if collectable {
                return detach_to_background(jobs, &id, budget);
            }
            // 没有可查的任务表：转后台就成了「还在跑，但你永远拿不到」。退回旧的
            // kill + 说明，宁可诚实地失败。
            return finish_early(
                jobs,
                &id,
                format!(
                    "Error: 命令超时（前台等待 {}）已被终止。\
                     下面是终止前已产出的输出。",
                    human_budget(budget)
                ),
            )
            .await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 预算的人读形式。秒级预算用 `{:.0}s` 会把 500ms 印成「0s」，那看着像个 bug。
fn human_budget(budget: Duration) -> String {
    if budget < Duration::from_secs(1) {
        format!("{}ms", budget.as_millis())
    } else {
        format!("{:.0}s", budget.as_secs_f64())
    }
}

/// 前台预算到点：**不杀命令**，把它转成后台任务，带回已产出的输出与 job_id。
///
/// 杀掉再让模型重跑是双重浪费：那几分钟的工作扔了，重跑还要再花同样的时间，
/// 而且大概率再超时一次——`cargo build` 不会因为重跑就变快。命令已经过了权限门、
/// 进程还活着，留着它比杀掉严格更优。
///
/// 这里**不是错误**，所以开头不写 `Error:`：模型看到 `Error:` 的第一反应是重试或
/// 换路子，而这次它什么都没做错，只是命令比预算长。
///
/// 取消（用户按 Esc）仍然走 [`finish_early`] 杀掉——那是明确要它停。
fn detach_to_background(jobs: &Jobs, id: &str, budget: Duration) -> String {
    let out = jobs.snapshot(id).map(|s| s.output).unwrap_or_default();
    // 转不动只有一种情况：这一瞬间它自己跑完了。那就当正常完成，别报超时。
    if !jobs.detach(id) {
        jobs.forget(id);
        return out;
    }
    let waited = human_budget(budget);
    let head = format!(
        "[命令仍在运行，已转入后台]（前台等待 {waited} 到点）\n\n\
         job_id: {id}\n\n\
         进程没有被终止，还在继续跑。不要重跑这条命令——用 job 工具配 \
         job_ids=[\"{id}\"] 取后续输出，要停就用 kill_task。\n\
         下面是转入后台前已产出的输出。"
    );
    if out.trim().is_empty() || out.trim() == "(no output)" {
        head
    } else {
        format!("{head}\n{out}")
    }
}

/// 取消的收尾：杀掉命令，把已经产出的输出带回来，再把任务摘掉。
///
/// 旧实现在这里直接 `kill` 后返回一句错误字符串，从不读管道，模型拿不到任何
/// 已完成的工作。
async fn finish_early(jobs: &Jobs, id: &str, reason: String) -> String {
    let _ = jobs.kill(id).await;
    // kill 是发信号，run 任务还要收尾；给它一小段时间把尾巴写完。
    for _ in 0..25 {
        if jobs.is_done(id) != Some(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let out = jobs.snapshot(id).map(|s| s.output).unwrap_or_default();
    jobs.forget(id);
    if out.trim().is_empty() || out.trim() == "(no output)" {
        reason
    } else {
        format!("{reason}\n{out}")
    }
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "bash".into(),
        description: BASH_DESC.into(),
        parameters_json: BASH_PARAMS.into(),
    }
}

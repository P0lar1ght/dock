//! Background jobs (`ctx.jobs`). Bash `is_background` and `tool-jobs` use this.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use std::time::{Duration, SystemTime};

use cordis::{plugin, Inject, Plugin};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::names::{JOBS, SUBAGENTS, TOOLS};
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use crate::tools::task::{render_subagent, Subagents};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

/// 单个任务保留的输出上限。对齐 Grok 的 `output_byte_limit`（默认 20k chars）。
///
/// 没有上限时，一条 `cargo test` 就能把上百万字节灌进上下文；在管道死锁被修掉
/// 之前这个问题被死锁挡着，修完就会真的发生。
pub(crate) const MAX_OUTPUT_BYTES: usize = 20 * 1024;
/// 头部保留量：留住命令开头的上下文（编译目标、参数回显）。
const HEAD_BYTES: usize = 4 * 1024;
/// 尾部保留量：失败原因和汇总几乎总在结尾。
const TAIL_BYTES: usize = MAX_OUTPUT_BYTES - HEAD_BYTES;
/// 子进程退出后留给读端收尾的窗口。
///
/// 孙进程会继承管道（`sleep 60 &`），子进程退出后读端不一定立刻 EOF。给一个
/// 有界窗口，到点放弃剩余输出，绝不无限期挂着。
const DRAIN_GRACE: Duration = Duration::from_millis(200);

/// 流式累积的输出，带头尾双端上限。
///
/// 中间超出的部分直接丢弃，但 `total` 保持单调，所以截断提示里报的是真实字节数。
#[derive(Default)]
pub(crate) struct OutputBuf {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
    /// 落盘句柄与路径。**必须边跑边写**：`push` 里 `tail.drain` 是即时丢弃，
    /// 命令结束（或超时）时中间字节早已不在内存里，那时再 offload 只能落到
    /// 已经截断过的那一份。
    spill: Option<Spill>,
    /// 用作落盘文件名的任务 id。空串表示这个 buf 不落盘（单测里的裸 buf）。
    id: String,
}

/// 一个仍在写入的完整输出副本。
struct Spill {
    path: String,
    file: std::fs::File,
}

impl std::fmt::Debug for Spill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spill").field("path", &self.path).finish()
    }
}

impl OutputBuf {
    /// 带落盘能力的 buf。`id` 决定落盘文件名。
    pub(crate) fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// 超出内存预算时开始落盘：先把此刻的 head + tail 原样写下去（这一刻二者
    /// 合起来**就是**全部输出，一个字节都还没丢），之后每一块都追加。
    ///
    /// best-effort：开不了文件就当没有落盘能力，行为回到只留头尾。
    fn open_spill(&mut self) {
        if self.id.is_empty() {
            return;
        }
        let dir = cordis_base::tool_output::spill_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!(
            "{}.txt",
            cordis_base::tool_output::offload_stem(&self.id)
        ));
        let Ok(mut file) = std::fs::File::create(&path) else {
            return;
        };
        use std::io::Write;
        if file.write_all(&self.head).is_err() {
            return;
        }
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        if file.write_all(&tail).is_err() {
            return;
        }
        self.spill = Some(Spill {
            path: path.to_string_lossy().into_owned(),
            file,
        });
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        // 顺序要紧：先判「这一块会不会让我们开始丢字节」，在真丢之前把已有内容
        // 落盘，再把这一块追加进去。反过来就会漏掉刚好跨过阈值的那一段。
        if self.spill.is_none() && self.total + bytes.len() > HEAD_BYTES + TAIL_BYTES {
            self.open_spill();
        }
        if let Some(spill) = self.spill.as_mut() {
            use std::io::Write;
            let _ = spill.file.write_all(bytes);
        }
        self.total += bytes.len();
        let mut rest = bytes;
        if self.head.len() < HEAD_BYTES {
            let take = (HEAD_BYTES - self.head.len()).min(rest.len());
            self.head.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
        }
        if rest.is_empty() {
            return;
        }
        if rest.len() >= TAIL_BYTES {
            self.tail.clear();
            self.tail.extend(&rest[rest.len() - TAIL_BYTES..]);
            return;
        }
        let overflow = (self.tail.len() + rest.len()).saturating_sub(TAIL_BYTES);
        if overflow > 0 {
            self.tail.drain(..overflow);
        }
        self.tail.extend(rest);
    }

    pub(crate) fn render(&self) -> String {
        let mut s = String::from_utf8_lossy(trim_trailing_partial(&self.head)).into_owned();
        if self.tail.is_empty() {
            return s;
        }
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        let dropped = self.total - self.head.len() - self.tail.len();
        if dropped > 0 {
            // 有落盘副本时给路径，让模型能把省略的那段捞回来——而不是像原先
            // 那样叫它把命令改成写文件再跑一遍。
            let recover = match self.spill.as_ref() {
                Some(spill) => format!(
                    "完整输出已写入：{}，用 read_file 或 grep 读它取回省略的部分。",
                    spill.path
                ),
                None => "需要完整内容就把命令改成写进文件再分段读。".into(),
            };
            s.push_str(&format!(
                "\n\n[… 已截断：中间省略 {dropped} 字节，总输出 {} 字节。{recover} …]\n\n",
                self.total
            ));
        }
        s.push_str(&String::from_utf8_lossy(trim_leading_continuation(&tail)));
        s
    }

    /// 最后一行非空输出，用于主界面任务条的进度摘要。
    ///
    /// 不能用 `render()` 再取最后一行：任务条常驻、每 80ms 重绘一次，整份
    /// 拼接是每个任务最多 20KB 的白工。这里只从尾部往回扫到上一个换行。
    pub(crate) fn last_line(&self, max_bytes: usize) -> String {
        // 尾部缓冲为空说明总量还没超过头部上限，此时尾行在 head 里。
        let scan: Vec<u8> = if self.tail.is_empty() {
            self.head.clone()
        } else {
            self.tail.iter().copied().collect()
        };
        let end = scan
            .iter()
            .rposition(|b| !matches!(b, b'\n' | b'\r' | b' ' | b'\t'))
            .map(|i| i + 1)
            .unwrap_or(0);
        if end == 0 {
            return String::new();
        }
        let start = scan[..end]
            .iter()
            .rposition(|b| *b == b'\n' || *b == b'\r')
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut slice = &scan[start..end];
        if slice.len() > max_bytes {
            slice = trim_trailing_partial(&slice[..max_bytes]);
        }
        // 尾部缓冲是按字节滚动的，起点可能落在多字节字符中间。
        String::from_utf8_lossy(trim_leading_continuation(slice))
            .trim()
            .to_string()
    }
}

/// 任务条摘要里单行输出的字节上限。再长也没地方画。
pub(crate) const TAIL_LINE_BYTES: usize = 200;

/// 裁掉结尾被切断的 UTF-8 序列，避免 `from_utf8_lossy` 在拼接处吐替换字符。
fn trim_trailing_partial(bytes: &[u8]) -> &[u8] {
    // 一个 UTF-8 序列最长 4 字节，往回找不超过 3 个续接字节。
    let mut cut = bytes.len();
    for _ in 0..3 {
        if cut == 0 {
            break;
        }
        match std::str::from_utf8(&bytes[..cut]) {
            Ok(_) => return &bytes[..cut],
            Err(e) => cut = e.valid_up_to(),
        }
    }
    &bytes[..cut]
}

/// 裁掉开头的续接字节（`0b10xx_xxxx`）—— 尾部缓冲是按字节滚动的，起点可能落在
/// 一个多字节字符中间。
fn trim_leading_continuation(bytes: &[u8]) -> &[u8] {
    let skip = bytes
        .iter()
        .take(3)
        .take_while(|b| (*b & 0b1100_0000) == 0b1000_0000)
        .count();
    &bytes[skip..]
}

#[derive(Clone, Debug)]
pub struct JobSnapshot {
    pub id: String,
    pub command: String,
    pub done: bool,
    pub output: String,
    /// Tool-call description shown in the Grok tasks pane (`Task …` / `Monitor …`).
    pub description: Option<String>,
    /// True for `monitor` tool tasks. Watchers group in the tasks pane.
    pub is_monitor: bool,
    /// True while a **foreground** bash is still blocking the turn.
    ///
    /// Foreground commands live in the same table purely so the TUI can render
    /// their output while they run; they are not background tasks and are kept
    /// out of the tasks pane and out of the `job` tool's no-id listing.
    pub foreground: bool,
    pub start_time: SystemTime,
}

struct Job {
    command: String,
    output: Mutex<OutputBuf>,
    /// `exit 1` 之类的前缀。输出是流式追加的，前缀只能单独存、渲染时拼。
    exit_note: Mutex<Option<String>>,
    done: AtomicBool,
    /// kill 信号。run 任务自己 select 在它上面并调用 `child.kill()`。
    ///
    /// 旧实现把 `Child` 关在 `AsyncMutex` 里，`wait()` 持锁期间 `kill()` 会被
    /// 一直挡住；改成信号后两边不再共用锁。
    cancel: tokio::sync::watch::Sender<bool>,
    description: Option<String>,
    is_monitor: bool,
    foreground: AtomicBool,
    start_time: SystemTime,
}

/// Named `"jobs"` service. Live-lookup; do not capture the Arc.
pub struct Jobs {
    seq: AtomicU64,
    inner: Mutex<HashMap<String, Arc<Job>>>,
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            seq: AtomicU64::new(1),
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn start(&self, command: impl Into<String>) -> String {
        self.start_ex(command, None, false)
    }

    pub fn start_ex(
        &self,
        command: impl Into<String>,
        description: Option<String>,
        is_monitor: bool,
    ) -> String {
        self.spawn_job(command, description, is_monitor, false, None)
    }

    /// `start_ex` 加一个工作目录。`bash` 的 `workdir` 参数走这里——让模型传
    /// 目录比让它写 `cd x && ...` 可靠：每次调用都是全新的 shell，`cd` 不留存，
    /// 拼在命令里还会跟 `&&` / 引号纠缠。
    pub fn start_ex_in(
        &self,
        command: impl Into<String>,
        description: Option<String>,
        is_monitor: bool,
        cwd: Option<PathBuf>,
    ) -> String {
        self.spawn_job(command, description, is_monitor, false, cwd)
    }

    /// Register a **foreground** bash so the TUI can show its output while it
    /// runs. The caller owns the lifetime: wait on the snapshot, then `forget`.
    pub fn start_foreground(&self, command: impl Into<String>) -> String {
        self.spawn_job(command, None, false, true, None)
    }

    /// `start_foreground` 加描述与工作目录。
    pub fn start_foreground_ex(
        &self,
        command: impl Into<String>,
        description: Option<String>,
        cwd: Option<PathBuf>,
    ) -> String {
        self.spawn_job(command, description, false, true, cwd)
    }

    /// Drop a finished foreground command from the table so it stops holding
    /// its output buffer for the rest of the session.
    pub fn forget(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }

    /// 把一条还在跑的前台命令转成后台任务：**进程不动**，只是不再阻塞这一轮。
    ///
    /// 前台预算到点时用它替代 `kill`。命令本身已经过了权限门、也已经跑了几分钟，
    /// 杀掉等于把那几分钟扔了、还逼模型原样重跑一次；转后台则是把它留在 `Jobs`
    /// 里，`job` 工具随时能收。
    ///
    /// 返回 `false` = 没有这个 id（已经收尾并被 `forget` 掉了）。
    pub fn detach(&self, id: &str) -> bool {
        match self.inner.lock().unwrap().get(id) {
            Some(job) => {
                // `foreground` 决定它算不算「挡着这一轮」：`live_count` 与 tasks
                // pane 都看这一位。
                job.foreground.store(false, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    fn spawn_job(
        &self,
        command: impl Into<String>,
        description: Option<String>,
        is_monitor: bool,
        foreground: bool,
        cwd: Option<PathBuf>,
    ) -> String {
        let command = command.into();
        let id = format!("job-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        // 接收端必须在 spawn 之前就建好：`subscribe()` 只看得到订阅之后的变更，
        // 放到 run_job 里订阅的话，早于它执行的 kill 信号会被整个丢掉。
        let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
        let job = Arc::new(Job {
            command: command.clone(),
            output: Mutex::new(OutputBuf::with_id(&id)),
            exit_note: Mutex::new(None),
            done: AtomicBool::new(false),
            cancel,
            description,
            is_monitor,
            foreground: AtomicBool::new(foreground),
            start_time: SystemTime::now(),
        });
        self.inner.lock().unwrap().insert(id.clone(), job.clone());
        tokio::spawn(run_job(job, command, cancel_rx, cwd));
        id
    }

    fn snap(id: &str, job: &Job) -> JobSnapshot {
        let body = job.output.lock().unwrap().render();
        let note = job.exit_note.lock().unwrap().clone();
        let output = match note {
            Some(note) if body.trim().is_empty() => note,
            Some(note) => format!("{note}\n{body}"),
            None => body,
        };
        JobSnapshot {
            id: id.into(),
            command: job.command.clone(),
            done: job.done.load(Ordering::Relaxed),
            output,
            description: job.description.clone(),
            is_monitor: job.is_monitor,
            foreground: job.foreground.load(Ordering::Relaxed),
            start_time: job.start_time,
        }
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(id, job)| Self::snap(id, job))
            .collect()
    }

    /// 与 [`Jobs::list`] 同形，但 `output` 只放最后一行。
    ///
    /// 给主界面任务条用：它常驻、每 80ms 重绘，走 `list()` 的话每帧都要把每个
    /// 任务最多 20KB 的输出整份拼出来再克隆，只为显示一行进度摘要。
    pub fn list_brief(&self) -> Vec<JobSnapshot> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(id, job)| {
                let tail = job.output.lock().unwrap().last_line(TAIL_LINE_BYTES);
                JobSnapshot {
                    id: id.clone(),
                    command: job.command.clone(),
                    done: job.done.load(Ordering::Relaxed),
                    output: tail,
                    description: job.description.clone(),
                    is_monitor: job.is_monitor,
                    foreground: job.foreground.load(Ordering::Relaxed),
                    start_time: job.start_time,
                }
            })
            .collect()
    }

    /// 任务条只需要知道「有没有活着的后台任务」，不必建快照。
    pub fn live_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|job| {
                !job.done.load(Ordering::Relaxed) && !job.foreground.load(Ordering::Relaxed)
            })
            .count()
    }

    /// 只读完成位，不碰输出缓冲。
    ///
    /// 前台 bash 每 20ms 轮询一次；走 `snapshot()` 的话每次都要 `render()` 把
    /// 头尾拼成一个最大 20KB 的 String，一条 30s 的命令要白拼 1500 次，只为读
    /// 一个 bool。
    pub(crate) fn is_done(&self, id: &str) -> Option<bool> {
        self.inner
            .lock()
            .unwrap()
            .get(id)
            .map(|job| job.done.load(Ordering::Relaxed))
    }

    pub fn snapshot(&self, id: &str) -> Option<JobSnapshot> {
        self.inner
            .lock()
            .unwrap()
            .get(id)
            .map(|job| Self::snap(id, job))
    }

    pub async fn wait(&self, ids: &[String], timeout_ms: u64) -> Vec<JobSnapshot> {
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.max(1));
        loop {
            let snaps: Vec<_> = ids.iter().filter_map(|id| self.snapshot(id)).collect();
            if snaps.iter().all(|s| s.done) || tokio::time::Instant::now() >= deadline {
                return if snaps.is_empty() {
                    ids.iter()
                        .map(|id| JobSnapshot {
                            id: id.clone(),
                            command: String::new(),
                            done: true,
                            output: format!(
                                "Task {id} not found. No background tasks exist in this session."
                            ),
                            description: None,
                            is_monitor: false,
                            foreground: false,
                            start_time: SystemTime::UNIX_EPOCH,
                        })
                        .collect()
                } else {
                    snaps
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub async fn kill(&self, id: &str) -> String {
        let job = self.inner.lock().unwrap().get(id).cloned();
        let Some(job) = job else {
            return format!(
                "Task or subagent {id} not found. No background tasks or subagents exist in this session."
            );
        };
        if job.done.load(Ordering::Relaxed) {
            return format!("{id} already finished");
        }
        // 只发信号：run 任务自己 kill 并收尾。这样 kill 不会被运行中的 wait 挡住。
        let _ = job.cancel.send(true);
        format!("killed {id}")
    }
}

/// 把一个读端持续抽干到共享缓冲。stdout / stderr 各跑一份，必须并发。
async fn pump<R: AsyncRead + Unpin>(reader: Option<R>, job: Arc<Job>) {
    let Some(mut reader) = reader else { return };
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => job.output.lock().unwrap().push(&chunk[..n]),
        }
    }
}

async fn run_job(
    job: Arc<Job>,
    command: String,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    cwd: Option<PathBuf>,
) {
    let mut builder = tokio::process::Command::new("bash");
    builder
        .arg("-lc")
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd.as_deref() {
        builder.current_dir(cwd);
    }
    let spawned = builder.spawn();
    let mut child = match spawned {
        Err(e) => {
            job.output
                .lock()
                .unwrap()
                .push(format!("Error spawning bash: {e}").as_bytes());
            job.done.store(true, Ordering::Relaxed);
            return;
        }
        Ok(child) => child,
    };

    // stdout 与 stderr 必须并发抽干。任一侧写满管道缓冲（64KB）子进程就会阻塞，
    // 而子进程不退出另一侧永远读不到 EOF —— 旧实现顺序 `read_to_end` 正是这样
    // 把 `cargo build` 这类往 stderr 猛写的命令永久挂死的。
    let out = tokio::spawn(pump(child.stdout.take(), job.clone()));
    let err = tokio::spawn(pump(child.stderr.take(), job.clone()));
    let (abort_out, abort_err) = (out.abort_handle(), err.abort_handle());

    let mut killed = false;
    let status = tokio::select! {
        st = child.wait() => st.ok(),
        _ = cancel.changed() => {
            killed = true;
            let _ = child.kill().await;
            child.wait().await.ok()
        }
    };

    // 子进程退出不代表管道关闭：孙进程（`sleep 60 &`）继承了写端就还开着。
    let drain = async {
        let _ = tokio::join!(out, err);
    };
    if tokio::time::timeout(DRAIN_GRACE, drain).await.is_err() {
        abort_out.abort();
        abort_err.abort();
    }

    // 我们自己发的 SIGKILL 不是命令的失败原因，报 `exit signal: 9` 只会误导。
    if let Some(st) = status {
        if !st.success() && !killed {
            *job.exit_note.lock().unwrap() = Some(format!("exit {st}"));
        }
    }
    if job.output.lock().unwrap().total == 0 && job.exit_note.lock().unwrap().is_none() {
        job.output.lock().unwrap().push(b"(no output)");
    }
    job.done.store(true, Ordering::Relaxed);
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

pub fn jobs() -> Plugin {
    plugin("jobs", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(JOBS, Jobs::new())?))
    })
}

const JOB_PARAMS: &str = r#"{"type":"object","properties":{"job_ids":{"type":"array","items":{"type":"string"},"description":"Job ids, as printed by the tool that started them (\"job-7\") or a subagent_id. Omit to list every job in this session."},"timeout_ms":{"type":"integer","description":"Wait up to this many ms for the named jobs to finish; omit or 0 for an immediate snapshot."}}}"#;
const KILL_PARAMS: &str =
    r#"{"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"]}"#;

pub fn tool_jobs() -> Plugin {
    plugin("tool-jobs", Inject::from([TOOLS, JOBS]), |ctx, _: &()| {
        let tools = ctx.require::<Tools>(TOOLS)?;
        let output: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { job_output(&ctx, call).await })
            })
        };
        let kill: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { kill_task(&ctx, call).await })
            })
        };
        own_registered(
                ctx,
                vec![
                    tools.register(
                        ToolSpec {
                            name: "job".into(),
                            description: "List background jobs, or read output and status from specific ones. Covers background terminal commands and subagents.\n- Omit job_ids to list every job in this session: one line each with status, how long it has been running, and the command, without output bodies. Use this when you have lost track of what is still running, then kill_task the ones you no longer need.\n- Pass job_ids to read their output: ids come from is_background=true commands, from a command that outlived its foreground budget, or from a task spawn subagent_id.\n- timeout_ms waits that long for the named jobs to finish; omit it or pass 0 for an immediate snapshot. Waiting never terminates a job — a timed-out wait returns the output so far and says so.\n- A subagent pushes its turn end to you, so poll only when you need the result now.".into(),
                            parameters_json: JOB_PARAMS.into(),
                        },
                        output,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "kill_task".into(),
                            description: "Terminate a running background terminal command, or dispose a live subagent. Use job with no job_ids first if you are not sure what is still running. For a subagent, use interrupt_agent instead when you only want to stop its current turn.".into(),
                            parameters_json: KILL_PARAMS.into(),
                        },
                        kill,
                    )?,
                ],
            )?;
        Ok(None)
    })
}

/// `job_ids` 是现在的名字，`task_ids` / `task_id` 是旧名。
///
/// 继续认旧名不是为了兼容磁盘格式，是为了模型：`get_task_output` / `wait_tasks`
/// 存在了很久，`/resume` 回来的历史里全是 `task_ids`，模型照着抄一遍是常态。
/// 认下来比回一句「参数名错了」便宜。
const ID_KEYS: [&str; 2] = ["job_ids", "task_ids"];
const ID_KEYS_SINGULAR: [&str; 2] = ["job_id", "task_id"];

fn parse_ids(raw: &str) -> (Vec<String>, u64) {
    let v: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let mut ids = Vec::new();
    for key in ID_KEYS {
        let Some(arr) = v.get(key).and_then(|x| x.as_array()) else {
            continue;
        };
        for item in arr {
            if let Some(s) = item.as_str() {
                if !s.is_empty() {
                    ids.push(s.to_string());
                }
            }
        }
        if !ids.is_empty() {
            break;
        }
    }
    if ids.is_empty() {
        for key in ID_KEYS_SINGULAR {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    ids.push(s.to_string());
                    break;
                }
            }
        }
    }
    let timeout = v.get("timeout_ms").and_then(|x| x.as_u64()).unwrap_or(0);
    (ids, timeout)
}

fn render_snap(s: &JobSnapshot) -> String {
    let status = if s.done { "done" } else { "running" };
    format!(
        "[{status}{}] {} {}\n{}",
        elapsed_suffix(s),
        s.id,
        s.command,
        s.output
    )
}

/// `[running 12m04s]`——「跑了多久」是模型判断一条后台任务还有没有用的唯一依据。
///
/// 只给还在跑的加：已完成的任务，耗时不影响任何决定。
fn elapsed_suffix(s: &JobSnapshot) -> String {
    if s.done {
        return String::new();
    }
    match s.start_time.elapsed() {
        Ok(d) => {
            let secs = d.as_secs();
            if secs >= 60 {
                format!(" {}m{:02}s", secs / 60, secs % 60)
            } else {
                format!(" {secs}s")
            }
        }
        // 系统时钟往回跳过：宁可不印，也不印一个负数似的怪值。
        Err(_) => String::new(),
    }
}

/// 清单里的一行：没有输出正文，只有状态 / 已跑多久 / id / 命令。
///
/// 清点「还有什么在跑」不该把每个任务最多 20KB 的输出全倒进上下文——模型付一次
/// 那个代价就不会再清点第二次，而这是它唯一能发现自己忘了哪个任务的途径。
fn render_brief(s: &JobSnapshot) -> String {
    let status = if s.done { "done" } else { "running" };
    let tail = s.output.trim();
    let tail = if tail.is_empty() {
        String::new()
    } else {
        format!("\n    {tail}")
    };
    format!(
        "[{status}{}] {} {}{tail}",
        elapsed_suffix(s),
        s.id,
        s.command
    )
}

async fn job_output(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let (ids, timeout) = parse_ids(&call.arguments);
    let jobs = ctx.get::<Jobs>(JOBS);
    let sub = ctx.get::<Subagents>(SUBAGENTS);
    if ids.is_empty() {
        let mut parts = Vec::new();
        if let Some(jobs) = &jobs {
            // 前台命令只是为了让 TUI 看见进度才挂在同一张表上，不是后台任务。
            // `list_brief` 而不是 `list`：清点用不着每条任务的完整输出，见
            // [`render_brief`]。
            parts.extend(
                jobs.list_brief()
                    .iter()
                    .filter(|j| !j.foreground)
                    .map(render_brief),
            );
        }
        if let Some(sub) = &sub {
            parts.extend(sub.list().iter().map(render_subagent));
        }
        if parts.is_empty() {
            return tool_result(call, "No background tasks exist in this session.");
        }
        return tool_result(call, parts.join("\n\n"));
    }
    let body = collect_output(jobs.as_deref(), sub.as_deref(), &ids, timeout).await;
    tool_result(call, with_wait_note(body))
}

async fn kill_task(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let (ids, _) = parse_ids(&call.arguments);
    let Some(id) = ids.first() else {
        return tool_result(call, "Error: task_id is required");
    };
    if let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) {
        if let Some(msg) = sub.kill(id) {
            return tool_result(call, msg);
        }
    }
    let Some(jobs) = ctx.get::<Jobs>(JOBS) else {
        return tool_result(call, "Error: jobs is not mounted");
    };
    let msg = jobs.kill(id).await;
    tool_result(call, msg)
}

/// 一次 `collect_output` 的收尾状态，决定正文后面补不补说明、补哪句。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WaitOutcome {
    /// 等到点了仍有任务在跑（不是「等到任务完成」）。
    timed_out: bool,
    /// 仍在跑的里面有子代理 —— 它会自己推回合结束，不必轮询。
    subagent_pending: bool,
}

/// 等到点了任务还没跑完时补一句：上面是**快照**不是结论，任务仍在跑。
///
/// 不加这句时，一个 `[running]` 快照很容易被当成最终结果读掉。turn-end 那半句
/// 只在真的有子代理在跑时给：后台 bash 和 monitor 不推通知，让它们去等一个永远
/// 不来的通知，等于换个方向再骗一次。
fn with_wait_note((body, outcome): (String, WaitOutcome)) -> String {
    if !outcome.timed_out {
        return body;
    }
    let mut note = String::from(
        "Still running when the wait timed out. The snapshot above is the output so far, not a \
         result — call again later.",
    );
    if outcome.subagent_pending {
        note.push_str(
            " A subagent pushes its turn end to you, so waiting for that notice works too.",
        );
    }
    format!("{body}\n\n{note}")
}

/// 收集 `ids` 的输出，返回正文与本次等待的收尾状态。
///
/// `timeout_ms` 是**本次调用愿意等多久**，不是任务的生命周期：到点返回的是当前
/// 快照（已产出输出 + `[running]`），任务照跑，下次调用接着查 —— 长任务不会因为
/// 某次等待到期而丢结果。`timeout_ms == 0` 是不等待的即时快照。
async fn collect_output(
    jobs: Option<&Jobs>,
    sub: Option<&Subagents>,
    ids: &[String],
    timeout_ms: u64,
) -> (String, WaitOutcome) {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(if timeout_ms == 0 { 1 } else { timeout_ms });
    loop {
        let mut parts = Vec::new();
        let mut all_done = true;
        let mut subagent_pending = false;
        for id in ids {
            if let Some(s) = sub.and_then(|s| s.snapshot(id)) {
                if s.running() {
                    all_done = false;
                    subagent_pending = true;
                }
                parts.push(render_subagent(&s));
            } else if let Some(j) = jobs.and_then(|j| j.snapshot(id)) {
                if !j.done {
                    all_done = false;
                }
                parts.push(render_snap(&j));
            } else {
                parts.push(format!(
                    "Task {id} not found. No background tasks or subagents exist in this session."
                ));
            }
        }
        if timeout_ms == 0 || all_done || tokio::time::Instant::now() >= deadline {
            // The parent has the result now, so drop the redundant turn-end
            // notice for whatever it just collected.
            if let Some(sub) = sub {
                for id in ids {
                    if sub.snapshot(id).is_some_and(|s| !s.running()) {
                        sub.consume_completion(id);
                    }
                }
            }
            let timed_out = timeout_ms > 0 && !all_done;
            return (
                parts.join("\n\n"),
                WaitOutcome {
                    timed_out,
                    subagent_pending: timed_out && subagent_pending,
                },
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清点「还有什么在跑」是模型发现自己忘了哪个后台任务的唯一途径，所以这一行
    /// 必须**便宜**（不带输出正文）且**说得出已跑多久**——否则它分不清 `[running]`
    /// 是 3 秒还是 40 分钟，也就无从判断该不该 kill。
    #[test]
    fn the_inventory_line_is_cheap_and_says_how_long_it_has_run() {
        let snap = JobSnapshot {
            id: "job-7".into(),
            command: "cargo build --release".into(),
            description: None,
            output: "Compiling serde".into(),
            done: false,
            is_monitor: false,
            foreground: false,
            start_time: std::time::SystemTime::now() - Duration::from_secs(724),
        };
        let line = render_brief(&snap);
        assert!(line.contains("job-7"), "{line}");
        assert!(line.contains("cargo build --release"), "{line}");
        assert!(
            line.contains("[running 12m04s]"),
            "要说得出跑了多久：{line}"
        );

        // 已完成的任务不印耗时：那个数字不影响任何决定。
        let done = JobSnapshot { done: true, ..snap };
        assert_eq!(
            render_brief(&done)
                .lines()
                .next()
                .unwrap()
                .split(']')
                .next(),
            Some("[done"),
            "{}",
            render_brief(&done)
        );
    }

    /// S2：超出内存预算的输出要边跑边落盘，**一个字节都不能丢**。
    ///
    /// `push` 里的 `tail.drain` 是即时丢弃，所以落盘必须在跨过阈值之前发生；
    /// 晚一步中间那段就永远回不来了。这条用例正是钉住这个时序。
    #[test]
    fn oversized_output_spills_every_byte_while_running() {
        let _env = cordis_base::test_env::scoped().home();
        let mut buf = OutputBuf::with_id("spill-job");
        // 每块 1KB，总量远超 HEAD+TAIL，逼它在中途开始落盘。
        let mut expected = Vec::new();
        for i in 0..40u32 {
            let chunk = format!("{i:04}{}\n", "x".repeat(1_019));
            buf.push(chunk.as_bytes());
            expected.extend_from_slice(chunk.as_bytes());
        }
        let rendered = buf.render();
        assert!(rendered.contains("已截断"), "{rendered}");
        let path = cordis_base::tool_output::spill_dir().join(format!(
            "{}.txt",
            cordis_base::tool_output::offload_stem("spill-job")
        ));
        assert!(
            rendered.contains(&path.to_string_lossy().to_string()),
            "截断提示要给出落盘路径：{rendered}"
        );
        let on_disk = std::fs::read(&path).expect("完整输出应已落盘");
        assert_eq!(
            on_disk.len(),
            expected.len(),
            "落盘副本必须是完整输出，不能有丢失"
        );
        assert_eq!(on_disk, expected, "落盘内容要与原始字节逐字一致");
    }

    /// 没超预算就不该建落盘文件——绝大多数命令都在这条路径上，
    /// 每条都写一个文件是纯粹的磁盘垃圾。
    #[test]
    fn small_output_does_not_spill() {
        let _env = cordis_base::test_env::scoped().home();
        let mut buf = OutputBuf::with_id("small-job");
        buf.push(b"hello\n");
        let rendered = buf.render();
        assert_eq!(rendered, "hello\n");
        assert!(!cordis_base::tool_output::spill_dir()
            .join(format!(
                "{}.txt",
                cordis_base::tool_output::offload_stem("small-job")
            ))
            .exists());
    }

    /// 到点仍有任务在跑：正文给 running 快照（任务没被中止），并补一句说明它不是结论。
    ///
    /// 后台 bash / monitor 不推回合结束通知，所以这句里不能出现 turn-end。
    #[tokio::test]
    async fn collect_output_reports_when_the_wait_runs_out() {
        let jobs = Jobs::new();
        let id = jobs.start("sleep 5");
        let started = std::time::Instant::now();
        let (body, outcome) =
            collect_output(Some(&jobs), None, std::slice::from_ref(&id), 300).await;
        // 先收进程再断言，免得中途失败漏一个 sleep 出去。
        assert!(jobs.kill(&id).await.contains("killed"), "收不掉测试进程");

        assert!(outcome.timed_out, "300ms 到点时 sleep 5 还在跑：{body}");
        assert!(!outcome.subagent_pending, "跑的是 bash job，不是子代理");
        // `[running` 而不是 `[running]`：状态后面还跟着已运行时长。
        assert!(body.contains("[running"), "{body}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "等待该按 timeout 返回，而不是挂到任务结束"
        );
        let note = with_wait_note((body, outcome));
        assert!(note.contains("not a result"), "{note}");
        assert!(
            !note.contains("turn end"),
            "bash job 没有 turn-end 通知：{note}"
        );
    }

    /// 等到点的是子代理时才提「它会自己推回合结束」。
    #[test]
    fn subagent_wait_note_points_at_the_turn_end_notice() {
        let outcome = WaitOutcome {
            timed_out: true,
            subagent_pending: true,
        };
        let note = with_wait_note(("[running] sub-1".into(), outcome));
        assert!(note.contains("not a result"), "{note}");
        assert!(note.contains("turn end"), "{note}");
    }

    /// 跑完的任务不报「等到点了」，也不加提示。
    #[tokio::test]
    async fn completed_task_is_not_a_timed_out_wait() {
        let jobs = Jobs::new();
        let id = jobs.start("echo done");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !jobs.snapshot(&id).is_some_and(|s| s.done) {
            assert!(std::time::Instant::now() < deadline, "echo 没在 10s 内跑完");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let (body, outcome) =
            collect_output(Some(&jobs), None, std::slice::from_ref(&id), 300).await;
        assert_eq!(outcome, WaitOutcome::default(), "{body}");
        assert!(body.contains("[done]"), "{body}");
        assert_eq!(
            with_wait_note((body.clone(), outcome)),
            body,
            "跑完了不该加提示"
        );
    }
}

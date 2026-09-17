//! 一次 workflow run 的 host 服务：预算记账、并发池、scratch、进度。
//!
//! 引擎（`vendor/xai/workflow`）把脚本里每次 `agent()` / `parallel()` 翻成一条
//! [`WorkflowHostRequest`]。**预算闸门完全由 host 的 `ReserveAgentCalls` 回执决
//! 定**（`engine.rs` 的 `reserve_agent_calls`：`agent()` 预留 1，`parallel()` 一
//! 次预留整批），引擎自己不记账，所以配额必须在这里兑现。
//!
//! 并发同理：`admission.rs` 对 workflow owner 的子代理直接放行（"follow the
//! run's own pool"），那个 pool 就是这里的 [`WorkflowHost::slots`]。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cordis_base::types::{LogEvent, NoticeKind};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xai_workflow::{AgentOpts, AgentResult, BudgetState, HostError, WorkflowHostRequest};

use super::drain::WorkflowState;
use super::schema_contract::{
    compile_contract_schema, contract_prompt, retry_prompt, validate_contract_output,
    SCHEMA_CONTRACT_RETRIES,
};
use crate::host::settings::ModelOverride;
use crate::names::{SESSIONS, SUBAGENTS};
use crate::session::log::Sessions;
use crate::tools::capability::CapabilityMode;
use crate::tools::task::types::SubagentRuntimeOverrides;
use crate::tools::task::{Subagents, WorkflowSpawn};

/// `workflow` 工具没传 `agent_budget` 时的默认上限，与工具描述一致。
/// 上限由 [`super::grok_tool::WorkflowToolInput::validate`] 对着
/// `MAX_AGENT_BUDGET` 校验，host 侧不重复校验。
pub(super) const DEFAULT_AGENT_BUDGET: u64 = 128;

/// 一次 run 同时在跑的子代理上限的出厂值。
pub const DEFAULT_MAX_CONCURRENT_AGENTS: usize = 32;

/// host 收尾时最多等这么久让本次 run 的孩子落地。
const WORKFLOW_CHILD_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// scratch 配额。跑飞的脚本不该能写满盘，也不该能把一个文件撑到内存装不下。
const WORKFLOW_MAX_SCRATCH_FILES: usize = 64;
const WORKFLOW_MAX_SCRATCH_FILE_BYTES: usize = 10 * 1024 * 1024;
const WORKFLOW_MAX_SCRATCH_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const WORKFLOW_MAX_SCRATCH_NAME_BYTES: usize = 255;

/// 脚本能往快照里塞多少东西。没有上限的话，一句 `phase()` 就能把任意长的字符串
/// 顶进 TUI。
const WORKFLOW_MAX_PHASE_BYTES: usize = 256;
const WORKFLOW_MAX_LOG_BYTES: usize = 4 * 1024;
const WORKFLOW_MAX_LOG_LINES: usize = 50;

/// 模型目录查询抽成参数，是为了可测：`lookup_model` 读的是用户自己的
/// `config.toml`，单测里那张表是什么样没法假定。
fn llm_override_from(
    opts: &AgentOpts,
    run_id: &str,
    known_model: impl Fn(&str) -> bool,
) -> ModelOverride {
    let model = opts
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .and_then(|m| {
            if known_model(m) {
                return Some(m.to_string());
            }
            tracing::warn!(
                run_id = %run_id,
                model = %m,
                "workflow agent(model:) 不在本机模型目录里，这个子代理仍用父会话的模型"
            );
            None
        });
    ModelOverride {
        model,
        effort: opts
            .effort
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string),
        // 上游那几条 wire 的字段都是 u32；脚本写得再大也按 u32 上限收。
        max_output_tokens: opts
            .max_output_tokens
            .map(|n| n.min(u32::MAX as u64) as u32),
    }
}

/// 配置值按机器并行度收窄：小机器上少跑几个，而不是照配置硬上。
pub(super) fn max_concurrent_agents(configured: usize) -> usize {
    max_concurrent_agents_from(
        configured,
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_AGENTS),
    )
}

/// 默认值也记一条：小机器悄悄比默认跑得少，运维看不见就成了玄学。
fn max_concurrent_agents_from(configured: usize, parallelism: usize) -> usize {
    let clamp = parallelism.max(2);
    let requested = configured.max(1);
    if requested > clamp {
        tracing::info!(
            requested,
            clamped_to = clamp,
            "workflow agent concurrency clamped to machine parallelism"
        );
    }
    requested.min(clamp)
}

pub(super) struct WorkflowHostParams {
    pub run_id: String,
    pub ctx: cordis::Context,
    pub state: Arc<WorkflowState>,
    pub cancel: CancellationToken,
    pub agent_budget: u64,
    /// 这次 run 同时在跑的子代理上限；更宽的 fan-out 按到达顺序排队。
    pub max_concurrent_agents: usize,
    pub scratch: PathBuf,
}

/// 一次 `agent()` 调用在契约循环里要带的全部状态。
struct ContractRun {
    /// 第一次尝试的子代理 id。契约重试会换一个，并把进度行改挂过去。
    first_id: String,
    /// 这次调用在 `agents` 里的行号。
    row: usize,
    prompt: String,
    description: String,
    subagent_type: String,
    capability_mode: Option<CapabilityMode>,
    /// `agent(model:/effort:/max_output_tokens:)`。空 = 跟父会话的模型走。
    llm: ModelOverride,
}

pub(super) struct WorkflowHost {
    params: WorkflowHostParams,
    /// 预留即扣减：一个池子，引擎在取消或预算终止时用 `ReleaseAgentCalls` 把
    /// 没用掉的还回来（`engine.rs` 的 `release_agent_calls`）。
    used: Mutex<u64>,
    /// **真实**子代理调用次数（含 schema 重试）。预算算的是逻辑调用，这个计数
    /// 防的是重试把真实调用放大到预算之外。
    agent_runs: Mutex<u64>,
    slots: tokio::sync::Semaphore,
}

/// 收 host 请求直到引擎收尾或本次 run 被取消。
pub(super) async fn run(
    params: WorkflowHostParams,
    mut rx: mpsc::UnboundedReceiver<WorkflowHostRequest>,
) {
    let _ = std::fs::create_dir_all(&params.scratch);
    let slots = tokio::sync::Semaphore::new(params.max_concurrent_agents.max(1));
    let host = Arc::new(WorkflowHost {
        params,
        used: Mutex::new(0),
        agent_runs: Mutex::new(0),
        slots,
    });
    loop {
        let req = tokio::select! {
            req = rx.recv() => match req {
                Some(req) => req,
                None => break,
            },
            _ = host.params.cancel.cancelled() => {
                while let Ok(req) = rx.try_recv() {
                    reply_cancelled(req);
                }
                break;
            }
        };
        host.clone().dispatch(req);
    }
    host.cancel_and_drain_children().await;
}

impl WorkflowHost {
    fn dispatch(self: Arc<Self>, req: WorkflowHostRequest) {
        match req {
            WorkflowHostRequest::ReserveAgentCalls { count, reply } => {
                let _ = reply.send(self.reserve(count));
            }
            WorkflowHostRequest::ReleaseAgentCalls { count, reply } => {
                self.release(count);
                let _ = reply.send(Ok(()));
            }
            WorkflowHostRequest::SpawnAgent { opts, reply } => {
                // 在这条请求入队的此刻钉死阶段：`agent()` 没写 `phase:` 就用
                // 当前 `phase()`。不能等到 `acquire_agent_slot` 之后再读——排队
                // 期间脚本已经可能 `phase()` 到下一段，planner 就会掉进「其它」。
                let mut opts = opts;
                if opts.phase.as_deref().is_none_or(|s| s.trim().is_empty()) {
                    opts.phase = self.current_phase();
                }
                let host = self.clone();
                tokio::spawn(async move {
                    let _ = reply.send(host.spawn_agent(opts).await);
                });
            }
            WorkflowHostRequest::Phase { title, .. } => {
                let title = truncate_bytes(title, WORKFLOW_MAX_PHASE_BYTES);
                self.patch(|snap| snap.current_phase = Some(title));
            }
            // 回放出来的那些之前已经记过，再记一遍就是重复。
            WorkflowHostRequest::Log {
                message,
                replayed: false,
            } => {
                let message = truncate_bytes(message, WORKFLOW_MAX_LOG_BYTES);
                tracing::info!(run_id = %self.params.run_id, "workflow log: {message}");
                self.patch(|snap| {
                    let logs = Arc::make_mut(&mut snap.logs);
                    if logs.len() >= WORKFLOW_MAX_LOG_LINES {
                        logs.remove(0);
                    }
                    logs.push(message);
                });
            }
            WorkflowHostRequest::Log { .. } => {}
            // dock 没有埋点通道。丢掉是对的，但别丢得无声无息。
            WorkflowHostRequest::Telemetry {
                name,
                replayed: false,
                ..
            } => {
                tracing::debug!(run_id = %self.params.run_id, event = %name, "workflow telemetry dropped: dock has no telemetry sink");
            }
            WorkflowHostRequest::Telemetry { .. } => {}
            WorkflowHostRequest::BudgetQuery { reply } => {
                let _ = reply.send(Ok(self.budget_state()));
            }
            WorkflowHostRequest::RenderTemplate { reply, .. } => {
                let _ = reply.send(Err(HostError::Unsupported(
                    "dock 不提供模板表，render_template 不可用".into(),
                )));
            }
            WorkflowHostRequest::WriteScratchFile {
                name,
                content,
                reply,
            } => {
                let host = self.clone();
                tokio::spawn(async move {
                    let _ = reply.send(host.write_scratch_file(&name, &content).await);
                });
            }
            WorkflowHostRequest::ReadScratchFile { name, reply } => {
                let host = self.clone();
                tokio::spawn(async move {
                    let _ = reply.send(host.read_scratch_file(&name).await);
                });
            }
            WorkflowHostRequest::GitDiffSince { reply, .. } => {
                let _ = reply.send(Err(HostError::Unsupported(
                    "dock 的仓库读写走 bash 权限门，git_diff_since 不可用".into(),
                )));
            }
        }
    }

    /// 预留一批子代理调用。超了就回 `AgentCallQuotaExceeded`，引擎会把它翻成
    /// `ControlToken::Budget` → `WorkflowOutcome::BudgetExceeded`。
    fn reserve(&self, count: u64) -> Result<(), HostError> {
        let maximum = self.params.agent_budget;
        {
            let mut used = self.used.lock().unwrap();
            let next = used
                .checked_add(count)
                .filter(|next| *next <= maximum)
                .ok_or(HostError::AgentCallQuotaExceeded {
                    requested: count,
                    maximum,
                })?;
            *used = next;
        }
        self.publish_budget();
        Ok(())
    }

    fn release(&self, count: u64) {
        {
            let mut used = self.used.lock().unwrap();
            *used = used.saturating_sub(count);
        }
        self.publish_budget();
    }

    fn budget_state(&self) -> BudgetState {
        let used = *self.used.lock().unwrap();
        let total = self.params.agent_budget;
        BudgetState {
            total: Some(total),
            spent: used,
            reserved: 0,
            remaining: Some(total.saturating_sub(used)),
        }
    }

    fn publish_budget(&self) {
        let used = *self.used.lock().unwrap();
        self.patch(|snap| snap.agents_used = used);
    }

    fn patch(&self, f: impl FnOnce(&mut super::WorkflowRunSnap)) {
        self.params.state.patch(&self.params.run_id, f);
    }

    /// 原子写一份 scratch 文件，返回它的绝对路径。
    ///
    /// 脚本会把这个路径直接交给用户（`deep_research.rhai` 的 `report.md` 就是），
    /// 所以中途崩了不能留半个文件：先写临时文件再 `rename`。
    async fn write_scratch_file(&self, name: &str, content: &str) -> Result<String, HostError> {
        if content.len() > WORKFLOW_MAX_SCRATCH_FILE_BYTES {
            return Err(HostError::Failed(format!(
                "scratch 文件超过 {WORKFLOW_MAX_SCRATCH_FILE_BYTES} 字节上限"
            )));
        }
        let name = scratch_component(name)?.to_owned();
        let dir = self.params.scratch.clone();
        let body = content.as_bytes().to_vec();
        // 同步文件 IO 不能留在 host 循环里：同一个循环还负责派发 SpawnAgent，
        // 写一份大 report 会把整个 host 卡住。
        tokio::task::spawn_blocking(move || {
            let path = dir.join(&name);
            reject_symlink(&dir, "scratch 目录")?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| HostError::Failed(format!("建 scratch 目录失败：{e}")))?;
            reject_symlink(&dir, "scratch 目录")?;
            reject_symlink(&path, "scratch 文件")?;

            let (other_files, other_bytes) = scratch_usage(&dir, &path)?;
            let replacing = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
            if other_files.saturating_add(usize::from(!replacing)) > WORKFLOW_MAX_SCRATCH_FILES {
                return Err(HostError::Failed(format!(
                    "scratch 文件数超过上限（最多 {WORKFLOW_MAX_SCRATCH_FILES} 个）"
                )));
            }
            if other_bytes.saturating_add(body.len() as u64) > WORKFLOW_MAX_SCRATCH_TOTAL_BYTES {
                return Err(HostError::Failed(format!(
                    "scratch 总字节超过上限（最多 {WORKFLOW_MAX_SCRATCH_TOTAL_BYTES} 字节）"
                )));
            }
            atomic_write(&dir, &path, &body)?;
            Ok(path.display().to_string())
        })
        .await
        .map_err(|e| HostError::Failed(format!("scratch 写入任务失败：{e}")))?
    }

    async fn read_scratch_file(&self, name: &str) -> Result<String, HostError> {
        let name = scratch_component(name)?.to_owned();
        let dir = self.params.scratch.clone();
        tokio::task::spawn_blocking(move || {
            let path = dir.join(&name);
            reject_symlink(&dir, "scratch 目录")?;
            reject_symlink(&path, "scratch 文件")?;
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| HostError::Failed(format!("读 scratch 文件失败：{e}")))?;
            if !meta.is_file() {
                return Err(HostError::Failed("scratch 路径不是普通文件".into()));
            }
            if meta.len() > WORKFLOW_MAX_SCRATCH_FILE_BYTES as u64 {
                return Err(HostError::Failed(format!(
                    "scratch 文件超过 {WORKFLOW_MAX_SCRATCH_FILE_BYTES} 字节读取上限"
                )));
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| HostError::Failed(format!("读 scratch 文件失败：{e}")))?;
            String::from_utf8(bytes)
                .map_err(|e| HostError::Failed(format!("scratch 文件不是 UTF-8：{e}")))
        })
        .await
        .map_err(|e| HostError::Failed(format!("scratch 读取任务失败：{e}")))?
    }

    /// 收尾：本次 run 的孩子一个都不许活过 host 循环。
    ///
    /// 引擎正常收尾时通常已经没有孩子了，这条是取消路径的兜底 —— 引擎一被取消
    /// 就返回，不会等在跑的 `agent()`。
    async fn cancel_and_drain_children(&self) {
        let Some(sub) = self.params.ctx.get::<Subagents>(SUBAGENTS) else {
            return;
        };
        sub.cancel_workflow_and_drain(&self.params.run_id, WORKFLOW_CHILD_DRAIN_TIMEOUT)
            .await;
    }

    /// 占一个 run 内的并发位。取消优先：排队中被取消就不再往下走。
    async fn acquire_agent_slot(&self) -> Result<tokio::sync::SemaphorePermit<'_>, HostError> {
        if self.params.cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        if let Ok(permit) = self.slots.try_acquire() {
            return Ok(permit);
        }
        tokio::select! {
            biased;
            _ = self.params.cancel.cancelled() => Err(HostError::Cancelled),
            permit = self.slots.acquire() => permit.map_err(|_closed| {
                debug_assert!(false, "agent slot semaphore is never closed");
                HostError::Cancelled
            }),
        }
    }

    async fn spawn_agent(&self, opts: AgentOpts) -> Result<AgentResult, HostError> {
        if self.params.cancel.is_cancelled() {
            return Err(HostError::Cancelled);
        }
        let Some(sub) = self.params.ctx.get::<Subagents>(SUBAGENTS) else {
            return Err(HostError::Failed("subagents 未挂载".into()));
        };
        if opts.prompt.trim().is_empty() {
            return Err(HostError::Failed("子代理 prompt 不能为空".into()));
        }
        // schema 编译失败是脚本写错了，现在就说，别等孩子跑完再说。
        let validator = match &opts.output_schema {
            None => None,
            Some(schema) => Some(compile_contract_schema(schema).map_err(HostError::Failed)?),
        };
        let prompt = match &opts.output_schema {
            None => opts.prompt.clone(),
            Some(schema) => contract_prompt(&opts.prompt, schema),
        };
        let description = opts
            .label
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "workflow-agent".into());
        let subagent_type = opts
            .agent_type
            .as_deref()
            .unwrap_or("general-purpose")
            .to_string();
        // 脚本要只读就得真的只读：`deep_research.rhai` 四处 `capability_mode:
        // "read-only"` 都是在明说"这个孩子要啃不可信的网页内容"。写错档位名
        // 现在就报错，而不是悄悄按不设限跑。
        let capability_mode = match opts.capability_mode.as_deref() {
            None => None,
            Some(raw) => Some(CapabilityMode::parse(raw).ok_or_else(|| {
                HostError::Failed(format!(
                    "capability_mode 只能是 read-only / read-write / execute / all，收到：{raw}"
                ))
            })?),
        };
        let llm = self.llm_override(&opts);

        // 先占位再记 running：排队中的子代理不该显示成在跑。整个契约循环共用
        // 一个位子，重试不额外占并发。
        let _slot = self.acquire_agent_slot().await?;
        let first_id = uuid::Uuid::now_v7().to_string();
        let row = self.agent_started(&first_id, &description, opts.phase.clone());
        self.patch(|snap| snap.agents_running = snap.agents_running.saturating_add(1));
        let outcome = {
            let work = self.run_contract_loop(
                &sub,
                ContractRun {
                    first_id,
                    row,
                    prompt,
                    description,
                    subagent_type,
                    capability_mode,
                    llm,
                },
                validator.as_ref(),
            );
            tokio::pin!(work);
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(80));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    outcome = &mut work => break outcome,
                    // 长跑时上报要马上进 overlay / 滚动区卡片，不能等孩子收尾。
                    _ = ticks.tick() => self.drain_reports(&sub),
                }
            }
        };
        self.patch(|s| s.agents_running = s.agents_running.saturating_sub(1));
        self.drain_reports(&sub);
        self.agent_finished(row, &outcome);
        outcome
    }

    /// `agent(model:/effort:/max_output_tokens:)` → 子会话的采样覆写。
    ///
    /// **模型必须在用户自己的目录（`config.toml`）里。** 这几个键是照 grok 的
    /// 脚本 API 写的，脚本里那个名字大概率是 grok 的模型；照单全收只会把这个
    /// 孩子打到一个解析不出端点的 id 上，收到 404 比忽略它糟得多。认不出来的
    /// 就退回父会话的模型，并记一条 warn 说清楚是哪一个。
    fn llm_override(&self, opts: &AgentOpts) -> ModelOverride {
        llm_override_from(opts, &self.params.run_id, |m| {
            cordis_base::config::lookup_model(m).is_some()
        })
    }

    fn current_phase(&self) -> Option<String> {
        self.params
            .state
            .list()
            .into_iter()
            .find(|r| r.run_id == self.params.run_id)
            .and_then(|r| r.current_phase)
            .filter(|s| !s.trim().is_empty())
    }

    /// 记一行「某个子代理开跑了」，返回它在 `agents` 里的下标。
    ///
    /// `agent(phase:)` 没写就回退到脚本当前 `phase()` 的那一段：脚本已经声明过
    /// 「现在在哪一阶段」，没必要每次 `agent()` 再抄一遍。`deep_research.rhai`
    /// 的 planner 就是只 `phase("Plan")` 没在 `agent()` 里带 `phase`。
    fn agent_started(&self, agent_id: &str, label: &str, phase: Option<String>) -> usize {
        let mut index = 0;
        self.patch(|snap| {
            let phase = phase
                .filter(|s| !s.trim().is_empty())
                .or_else(|| snap.current_phase.clone());
            let agents = Arc::make_mut(&mut snap.agents);
            index = agents.len();
            agents.push(super::drain::WorkflowAgentRow {
                agent_id: agent_id.to_owned(),
                label: label.to_owned(),
                phase,
                state: "running".into(),
                tokens_used: 0,
                duration_ms: 0,
                latest_report: None,
            });
        });
        index
    }

    /// 契约重试换了子代理 id：把进度行改挂到新的那个。
    fn rebind_agent(&self, index: usize, agent_id: &str) {
        self.patch(|snap| {
            let agents = Arc::make_mut(&mut snap.agents);
            if let Some(row) = agents.get_mut(index) {
                row.agent_id = agent_id.to_owned();
            }
        });
    }

    fn agent_finished(&self, index: usize, outcome: &Result<AgentResult, HostError>) {
        self.patch(|snap| {
            let agents = Arc::make_mut(&mut snap.agents);
            let Some(row) = agents.get_mut(index) else {
                return;
            };
            match outcome {
                Ok(result) => {
                    row.state = if result.cancelled {
                        "cancelled".into()
                    } else if result.success {
                        "done".into()
                    } else {
                        "failed".into()
                    };
                    row.tokens_used = result.tokens_used;
                    row.duration_ms = result.duration_ms;
                    if !result.success && row.latest_report.is_none() {
                        // 失败时脚本拿到的是 output 里的错误文本；详情页原先只画
                        // `failed · 0s`，原因被丢掉了。
                        row.latest_report = Some(match &result.output {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        });
                    }
                }
                Err(HostError::Cancelled) => row.state = "cancelled".into(),
                Err(err) => {
                    row.state = "failed".into();
                    row.latest_report = Some(err.to_string());
                }
            }
        });
    }

    /// 取走本次 run 攒下的子代理上报，折进对应的行与 run 的进度流。
    ///
    /// 这些上报**不进父信箱**（`ChildStore::push_report` 按 owner 分流），所以
    /// 不会在 run 跑到一半时叫醒主线程；收尾通知再把它们一并交出去。
    fn drain_reports(&self, sub: &Subagents) {
        let reports = sub.take_workflow_reports(&self.params.run_id);
        if reports.is_empty() {
            return;
        }
        // 收尾通知要带**每一条**上报：行上的 `latest_report` 是覆盖写的，同一个
        // 孩子报第二次就把第一次挤掉了，从它反推等于对主线程少说了一半。
        self.params
            .state
            .push_reports(&self.params.run_id, reports.iter().cloned());
        let mut cards: Vec<(String, String)> = Vec::new();
        self.patch(|snap| {
            let mut lines = Vec::new();
            {
                let agents = Arc::make_mut(&mut snap.agents);
                for report in reports {
                    let label = agents
                        .iter_mut()
                        .find(|row| row.agent_id == report.agent_id)
                        .map(|row| {
                            row.latest_report = Some(report.output.clone());
                            row.label.clone()
                        })
                        .unwrap_or_else(|| report.agent_id.clone());
                    cards.push((label.clone(), report.output.clone()));
                    lines.push(format!(
                        "{label}: {}",
                        truncate_bytes(report.output, WORKFLOW_MAX_LOG_BYTES)
                    ));
                }
            }
            let logs = Arc::make_mut(&mut snap.logs);
            for line in lines {
                if logs.len() >= WORKFLOW_MAX_LOG_LINES {
                    logs.remove(0);
                }
                logs.push(line);
            }
        });
        // 同一份上报在滚动区也出一张卡：用户该看得见 run 里发生了什么，而
        // `LogEvent::Notice` 不进模型历史，所以看得见不等于被打断。
        let Some(sessions) = self.params.ctx.get::<Sessions>(SESSIONS) else {
            return;
        };
        let name = self.run_name();
        for (label, body) in cards {
            sessions.append(LogEvent::Notice {
                kind: NoticeKind::WorkflowReport,
                title: format!("{name} · {label} 上报"),
                body,
            });
        }
    }

    fn run_name(&self) -> String {
        self.params
            .state
            .list()
            .into_iter()
            .find(|r| r.run_id == self.params.run_id)
            .map(|r| r.name)
            .unwrap_or_else(|| self.params.run_id.clone())
    }

    /// 跑一个子代理，契约不满足就 resume 重试一次。
    async fn run_contract_loop(
        &self,
        sub: &Subagents,
        run: ContractRun,
        validator: Option<&jsonschema::Validator>,
    ) -> Result<AgentResult, HostError> {
        let ContractRun {
            first_id,
            row,
            prompt,
            description,
            subagent_type,
            capability_mode,
            llm,
        } = run;
        let started = Instant::now();
        let mut attempts: u32 = 0;
        let mut total_tokens: u64 = 0;
        let mut total_duration: u64 = 0;
        let mut next_prompt = prompt;
        let mut resume_from: Option<String> = None;
        let mut child_id = first_id;

        loop {
            attempts += 1;
            // 重试会多花一次真实调用，但不再扣预算（预算算的是逻辑 agent 调用）。
            // 这道闸门防的是重试把真实调用放大到预算之外。
            if self.bump_agent_runs() > self.max_agent_runs() {
                return Err(HostError::Failed(format!(
                    "子代理实际调用次数超过上限（最多 {}）",
                    self.max_agent_runs()
                )));
            }
            let result = sub
                .spawn_for_workflow(WorkflowSpawn {
                    run_id: self.params.run_id.clone(),
                    id: child_id.clone(),
                    prompt: next_prompt.clone(),
                    description: description.clone(),
                    subagent_type: subagent_type.clone(),
                    resume_from: resume_from.take(),
                    // 用 run 自己的令牌：run 一取消，在跑的孩子立刻收到。
                    cancel_token: self.params.cancel.clone(),
                    runtime_overrides: SubagentRuntimeOverrides {
                        capability_mode,
                        llm: llm.clone(),
                        ..Default::default()
                    },
                })
                .await;
            total_tokens = total_tokens.saturating_add(result.total_tokens_used);
            total_duration = total_duration.saturating_add(result.duration_ms);
            if result.cancelled && self.params.cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }

            // 失败的孩子要如实说失败。脚本会把 output 当数据继续往下算，所以
            // 失败时交的是错误文本，`success` 必须是 false。
            let failed_output = || {
                serde_json::Value::String(
                    result
                        .error
                        .clone()
                        .unwrap_or_else(|| result.output.to_string()),
                )
            };
            let (success, output) = match validator {
                _ if !result.success => (false, failed_output()),
                None => (true, agent_output_value(&result.output)),
                Some(validator) => match validate_contract_output(validator, &result.output) {
                    Ok(value) => (true, value),
                    Err(err) if attempts <= SCHEMA_CONTRACT_RETRIES => {
                        // 从这个孩子的 transcript 续跑：它已经做完了活，缺的只是
                        // 把结论按格式写出来，重头再跑一遍是纯浪费。
                        resume_from = Some(result.child_session_id.clone());
                        next_prompt = retry_prompt(&err);
                        // 重试是同一行的第二次尝试：换 id 并改挂，别让上报对空。
                        child_id = uuid::Uuid::now_v7().to_string();
                        self.rebind_agent(row, &child_id);
                        continue;
                    }
                    Err(err) => (
                        false,
                        serde_json::Value::String(format!("结构化产出校验失败：{err}")),
                    ),
                },
            };
            return Ok(AgentResult {
                agent_id: result.subagent_id,
                success,
                output,
                cancelled: result.cancelled,
                tokens_used: total_tokens,
                duration_ms: if total_duration > 0 {
                    total_duration
                } else {
                    started.elapsed().as_millis() as u64
                },
            });
        }
    }

    /// 本次 run 允许的真实子代理调用次数上限：每个逻辑调用最多跑
    /// `1 + SCHEMA_CONTRACT_RETRIES` 次。
    fn max_agent_runs(&self) -> u64 {
        self.params
            .agent_budget
            .saturating_mul(u64::from(SCHEMA_CONTRACT_RETRIES) + 1)
    }

    fn bump_agent_runs(&self) -> u64 {
        let mut runs = self.agent_runs.lock().unwrap();
        *runs += 1;
        *runs
    }
}

fn agent_output_value(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return value;
    }
    if let Some(fenced) = extract_fenced_json(trimmed) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&fenced) {
            return value;
        }
    }
    serde_json::json!(raw)
}

fn extract_fenced_json(raw: &str) -> Option<String> {
    let after_open = raw.split_once("```")?.1;
    let body = after_open
        .strip_prefix("json")
        .or_else(|| after_open.strip_prefix("JSON"))
        .unwrap_or(after_open);
    let body = body
        .strip_prefix('\n')
        .or_else(|| body.strip_prefix("\r\n"))
        .unwrap_or(body);
    let inner = body.split_once("```")?.0.trim();
    if inner.starts_with('{') || inner.starts_with('[') {
        Some(inner.to_string())
    } else {
        None
    }
}

/// 按字节截断，落在字符边界上。超长时补一个省略号，免得看起来像是原文就这么短。
fn truncate_bytes(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    text
}

/// scratch 名字必须是**单个普通路径组件**。
///
/// 旧实现把非法字符换成 `_`，看着像是挡住了穿越，但 `"."` / `".."` / 空名字会
/// 落到目录本身或父目录，报错含糊。这里直接拒掉，错误说清原因。
fn scratch_component(name: &str) -> Result<&str, HostError> {
    if name.len() > WORKFLOW_MAX_SCRATCH_NAME_BYTES {
        return Err(HostError::Failed(format!(
            "scratch 文件名超过 {WORKFLOW_MAX_SCRATCH_NAME_BYTES} 字节"
        )));
    }
    let mut components = std::path::Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(part)), None) if !part.is_empty() => Ok(name),
        _ => Err(HostError::Failed(format!(
            "scratch 文件名必须是单个相对路径组件，收到：{name}"
        ))),
    }
}

/// 目录里现有的文件数与总字节，不算即将被替换的那一个。
fn scratch_usage(
    dir: &std::path::Path,
    replacing: &std::path::Path,
) -> Result<(usize, u64), HostError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(HostError::Failed(format!("列 scratch 目录失败：{e}"))),
    };
    let mut files = 0usize;
    let mut bytes = 0u64;
    for entry in entries {
        let entry = entry.map_err(|e| HostError::Failed(format!("读 scratch 目录项失败：{e}")))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| HostError::Failed(format!("读 scratch 元数据失败：{e}")))?;
        if meta.file_type().is_symlink() {
            return Err(HostError::Failed(format!(
                "scratch 目录里有符号链接：{}",
                path.display()
            )));
        }
        if meta.is_file() && path != replacing {
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(meta.len());
        }
    }
    Ok((files, bytes))
}

/// 先写同目录临时文件再 `rename`：中途崩了留下的是临时文件，不是半个 report。
fn atomic_write(
    dir: &std::path::Path,
    path: &std::path::Path,
    body: &[u8],
) -> Result<(), HostError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp = dir.join(format!(".dock-scratch-{stamp}-{}.tmp", std::process::id()));
    let write = std::fs::write(&tmp, body)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| HostError::Failed(format!("写 scratch 文件失败：{e}")));
    if write.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write
}

/// 拒绝符号链接：跟着链接写出去就等于让脚本改 scratch 目录外的任意文件。
fn reject_symlink(path: &std::path::Path, what: &str) -> Result<(), HostError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(HostError::Failed(format!(
            "{what}不能是符号链接：{}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(HostError::Failed(format!("读取{what}元数据失败：{e}"))),
    }
}

fn reply_cancelled(req: WorkflowHostRequest) {
    use WorkflowHostRequest as R;
    match req {
        R::ReserveAgentCalls { reply, .. } | R::ReleaseAgentCalls { reply, .. } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::SpawnAgent { reply, .. } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::BudgetQuery { reply } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::RenderTemplate { reply, .. }
        | R::WriteScratchFile { reply, .. }
        | R::ReadScratchFile { reply, .. }
        | R::GitDiffSince { reply, .. } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::Phase { .. } | R::Log { .. } | R::Telemetry { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_output_parses_json_object_or_fence() {
        let obj = agent_output_value(r#"{"questions":["a"]}"#);
        assert_eq!(obj["questions"][0], "a");
        let fenced = agent_output_value("here\n```json\n{\"claim\":\"x\"}\n```\n");
        assert_eq!(fenced["claim"], "x");
        let plain = agent_output_value("not json");
        assert_eq!(plain, serde_json::json!("not json"));
    }

    /// `agent(model:)` 点名的模型必须是**本机目录里真有的** id。
    ///
    /// 这几个键是照 grok 的脚本 API 写的，脚本里那个名字大概率是 grok 的模型。
    /// 照单全收 = 这个孩子被打到一个解析不出端点的 id 上，换来一个 404；退回
    /// 父会话的模型至少还跑得完。
    #[test]
    fn an_unknown_model_falls_back_to_the_parent_session() {
        let known = |m: &str| m == "local-fast";
        let opts = AgentOpts {
            model: Some("grok-4-fast".into()),
            effort: Some("low".into()),
            max_output_tokens: Some(4096),
            ..AgentOpts::default()
        };
        let over = llm_override_from(&opts, "wf_1", known);
        assert_eq!(over.model, None, "目录里没有的模型不该点名");
        assert_eq!(over.effort.as_deref(), Some("low"), "强度与模型无关，照带");
        assert_eq!(over.max_output_tokens, Some(4096));

        let opts = AgentOpts {
            model: Some(" local-fast ".into()),
            ..AgentOpts::default()
        };
        let over = llm_override_from(&opts, "wf_1", known);
        assert_eq!(over.model.as_deref(), Some("local-fast"));
        assert!(over.effort.is_none());
    }

    /// 一个都没点名就别挂这个服务：空覆写会让子代理走一条本可以不走的分支。
    #[test]
    fn an_empty_override_stays_empty() {
        let over = llm_override_from(&AgentOpts::default(), "wf_1", |_| true);
        assert!(over.is_empty());
        let blank = AgentOpts {
            model: Some("   ".into()),
            effort: Some(String::new()),
            ..AgentOpts::default()
        };
        assert!(
            llm_override_from(&blank, "wf_1", |_| true).is_empty(),
            "空白字符串不算点名"
        );
    }

    #[test]
    fn concurrency_clamps_to_machine_parallelism() {
        let from = max_concurrent_agents_from;
        assert_eq!(from(8, 64), 8);
        assert_eq!(from(64, 8), 8);
        assert_eq!(from(64, 1), 2);
        assert_eq!(from(0, 64), 1);
    }

    fn host(agent_budget: u64) -> WorkflowHost {
        host_in(
            agent_budget,
            std::env::temp_dir().join("dock-workflow-host-test"),
        )
    }

    fn host_in(agent_budget: u64, scratch: PathBuf) -> WorkflowHost {
        WorkflowHost {
            params: WorkflowHostParams {
                run_id: "wf_test".into(),
                ctx: cordis::Context::new(),
                state: Arc::new(WorkflowState::new()),
                cancel: CancellationToken::new(),
                agent_budget,
                max_concurrent_agents: 1,
                scratch,
            },
            used: Mutex::new(0),
            agent_runs: Mutex::new(0),
            slots: tokio::sync::Semaphore::new(1),
        }
    }

    /// 带 scratch 目录的 host。`TempDir` 要留着，drop 了目录就没了。
    fn scratch_host() -> (WorkflowHost, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let host = host_in(8, dir.path().join("scratch"));
        (host, dir)
    }

    #[tokio::test]
    async fn scratch_round_trips_a_file() {
        let (host, _dir) = scratch_host();
        let path = host.write_scratch_file("report.md", "hello").await.unwrap();
        assert!(path.ends_with("report.md"), "{path}");
        assert_eq!(host.read_scratch_file("report.md").await.unwrap(), "hello");
        // 覆盖写不该把配额算成两份。
        host.write_scratch_file("report.md", "again").await.unwrap();
        assert_eq!(host.read_scratch_file("report.md").await.unwrap(), "again");
    }

    /// 名字必须是单个路径组件。旧实现把 `/` 换成 `_`，`.` / `..` / 空名字则会
    /// 落到目录本身或父目录，报错含糊。
    #[tokio::test]
    async fn scratch_rejects_anything_but_one_component() {
        let (host, _dir) = scratch_host();
        for bad in ["", ".", "..", "a/b", "../escape", "/abs"] {
            let err = host.write_scratch_file(bad, "x").await.unwrap_err();
            assert!(
                matches!(&err, HostError::Failed(m) if m.contains("单个相对路径组件")),
                "{bad:?} 应当被拒：{err:?}"
            );
        }
        let long = "n".repeat(WORKFLOW_MAX_SCRATCH_NAME_BYTES + 1);
        assert!(host.write_scratch_file(&long, "x").await.is_err());
    }

    /// 跟着符号链接写出去 = 脚本能改 scratch 目录外的任意文件。
    #[cfg(unix)]
    #[tokio::test]
    async fn scratch_refuses_to_follow_a_symlink() {
        let (host, dir) = scratch_host();
        let scratch = dir.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "原样").unwrap();
        std::os::unix::fs::symlink(&outside, scratch.join("report.md")).unwrap();

        let err = host
            .write_scratch_file("report.md", "被劫持")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, HostError::Failed(m) if m.contains("符号链接")),
            "{err:?}"
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "原样");
        assert!(host.read_scratch_file("report.md").await.is_err());
    }

    #[tokio::test]
    async fn scratch_enforces_the_file_count_quota() {
        let (host, _dir) = scratch_host();
        for i in 0..WORKFLOW_MAX_SCRATCH_FILES {
            host.write_scratch_file(&format!("f{i}.txt"), "x")
                .await
                .unwrap();
        }
        let err = host.write_scratch_file("one-too-many.txt", "x").await;
        assert!(
            matches!(&err, Err(HostError::Failed(m)) if m.contains("文件数超过上限")),
            "{err:?}"
        );
        // 已有文件仍然可以覆盖写：配额卡的是新增。
        host.write_scratch_file("f0.txt", "y").await.unwrap();
    }

    #[tokio::test]
    async fn scratch_enforces_the_single_file_size_cap() {
        let (host, _dir) = scratch_host();
        let big = "x".repeat(WORKFLOW_MAX_SCRATCH_FILE_BYTES + 1);
        let err = host.write_scratch_file("big.txt", &big).await;
        assert!(
            matches!(&err, Err(HostError::Failed(m)) if m.contains("字节上限")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn reserve_stops_at_the_agent_budget() {
        let host = host(2);
        assert!(host.reserve(1).is_ok());
        assert!(host.reserve(1).is_ok());
        assert!(matches!(
            host.reserve(1),
            Err(HostError::AgentCallQuotaExceeded {
                requested: 1,
                maximum: 2
            })
        ));
        // 引擎在取消 / 预算终止时把没用掉的还回来，还回来的位子能再用。
        host.release(1);
        assert!(host.reserve(1).is_ok());
    }

    #[tokio::test]
    async fn a_whole_panel_is_rejected_before_any_child_launches() {
        let host = host(4);
        assert!(host.reserve(3).is_ok());
        assert!(
            matches!(
                host.reserve(2),
                Err(HostError::AgentCallQuotaExceeded {
                    requested: 2,
                    maximum: 4
                })
            ),
            "parallel() 预留整批：放不下就整批拒绝，不能先起一半"
        );
        assert_eq!(*host.used.lock().unwrap(), 3, "被拒的一批不记账");
    }

    #[tokio::test]
    async fn budget_query_reports_the_real_numbers() {
        let host = host(10);
        host.reserve(4).unwrap();
        let state = host.budget_state();
        assert_eq!(state.total, Some(10));
        assert_eq!(state.spent, 4);
        assert_eq!(state.remaining, Some(6));
    }
}

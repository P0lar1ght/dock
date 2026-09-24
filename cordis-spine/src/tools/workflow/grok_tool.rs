//! Copied from grok-build/.../xai-grok-tools/.../grok_build/workflow/mod.rs
//! (`WorkflowSource`, `WorkflowToolInput`, oneshot ack, `WorkflowTool::run`).
//! `xai_tool_runtime` / `register_resource!` stripped; drain lives in this crate.

pub const WORKFLOW_TOOL_NAME: &str = "workflow";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowSource {
    Name {
        #[schemars(
            description = "Name of a registered workflow (built-in, or discovered from the project `.dock/workflows/` or user `~/.dock/workflows/`)."
        )]
        name: String,
    },
    Script {
        #[schemars(
            description = "Inline Rhai workflow script. It must start with a pure-literal `let meta = #{ name: ..., description: ... };` map. Before authoring, read the `create-workflow` skill's SKILL.md. Run the path-specific `validate_only` smoke check with representative args."
        )]
        script: String,
    },
    ScriptPath {
        #[schemars(description = "Path to a .rhai workflow script on disk.")]
        script_path: String,
    },
    Resume {
        #[schemars(
            description = "Resume a same-process paused run, continuing its original immutable source and args. A budget-limited run resumes only when `agent_budget` is passed with a higher cap. Process-restart interruptions are terminal."
        )]
        resume_from_run_id: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct WorkflowToolInput {
    #[schemars(
        description = "Exactly one workflow source. The `type` tag selects a registered name, inline script, script path, or same-process resume."
    )]
    pub source: WorkflowSource,

    #[serde(default)]
    #[schemars(
        range(min = 1, max = 1024),
        description = "Absolute cumulative cap on logical child-agent calls for this run. Every agent() and every parallel() item consumes one slot; schema retries do not. Defaults to 128 and may be set from 1 through 1,024. A panel that would exceed the remaining budget is rejected before any of its children launch."
    )]
    pub agent_budget: Option<u64>,

    #[serde(default)]
    #[schemars(
        description = "JSON value bound to the script's `args` global. Use an object for named arguments."
    )]
    pub args: Option<serde_json::Value>,

    #[serde(default)]
    #[schemars(
        description = "Run a path-specific smoke check without launching: validate metadata, compile the full script, and execute the single path selected by the supplied args and canned host results. It does not exercise every branch or prove live tools and agent outputs work."
    )]
    pub validate_only: bool,
}

#[derive(serde::Deserialize)]
struct WorkflowToolInputWire {
    #[serde(default)]
    source: Option<WorkflowSource>,
    #[serde(default)]
    agent_budget: Option<u64>,
    #[serde(default)]
    args: Option<serde_json::Value>,
    #[serde(default)]
    validate_only: bool,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    script_path: Option<String>,
    #[serde(default)]
    resume_from_run_id: Option<String>,
}

impl<'de> serde::Deserialize<'de> for WorkflowToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let mut wire = WorkflowToolInputWire::deserialize(deserializer)?;
        wire.name = nonblank(wire.name);
        wire.script = nonblank(wire.script);
        wire.script_path = nonblank(wire.script_path);
        wire.resume_from_run_id = nonblank(wire.resume_from_run_id);
        let legacy_sources = [
            wire.name.is_some(),
            wire.script.is_some(),
            wire.script_path.is_some(),
            wire.resume_from_run_id.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if wire.source.is_some() && legacy_sources != 0 {
            return Err(D::Error::custom(
                "`source` cannot be combined with legacy `name`, `script`, `script_path`, or `resume_from_run_id` fields",
            ));
        }
        if legacy_sources > 1 {
            return Err(D::Error::custom(
                "workflow source fields are mutually exclusive; provide exactly one of `name`, `script`, `script_path`, or `resume_from_run_id`",
            ));
        }
        let source = wire
            .source
            .or_else(|| wire.name.map(|name| WorkflowSource::Name { name }))
            .or_else(|| wire.script.map(|script| WorkflowSource::Script { script }))
            .or_else(|| {
                wire.script_path
                    .map(|script_path| WorkflowSource::ScriptPath { script_path })
            })
            .or_else(|| {
                wire.resume_from_run_id
                    .map(|resume_from_run_id| WorkflowSource::Resume { resume_from_run_id })
            })
            .ok_or_else(|| {
                D::Error::custom(
                    "missing workflow source; provide `source` with exactly one of the `name`, `script`, `script_path`, or `resume` variants",
                )
            })?;
        Ok(Self {
            source,
            agent_budget: wire.agent_budget,
            args: wire.args,
            validate_only: wire.validate_only,
        })
    }
}

fn nonblank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

impl WorkflowToolInput {
    pub const MAX_AGENT_BUDGET: u64 = 1_024;

    pub fn normalize(&mut self) {
        match &mut self.source {
            WorkflowSource::Name { name } => *name = name.trim().to_owned(),
            WorkflowSource::Script { script } => {
                if script.trim().is_empty() {
                    script.clear();
                }
            }
            WorkflowSource::ScriptPath { script_path } => {
                *script_path = script_path.trim().to_owned();
            }
            WorkflowSource::Resume { resume_from_run_id } => {
                *resume_from_run_id = resume_from_run_id.trim().to_owned();
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(budget) = self.agent_budget {
            if budget == 0 {
                return Err("`agent_budget` must be a positive integer".into());
            }
            if budget > Self::MAX_AGENT_BUDGET {
                return Err(format!(
                    "`agent_budget` must be at most {} agents",
                    Self::MAX_AGENT_BUDGET
                ));
            }
        }
        let value = match &self.source {
            WorkflowSource::Name { name } => name,
            WorkflowSource::Script { script } => script,
            WorkflowSource::ScriptPath { script_path } => script_path,
            WorkflowSource::Resume { resume_from_run_id } => {
                if self.args.is_some() {
                    return Err(
                        "resume uses the original immutable source and arguments; do not provide `args`"
                            .into(),
                    );
                }
                if self.validate_only {
                    return Err("`validate_only` cannot be used when resuming a run".into());
                }
                resume_from_run_id
            }
        };
        if value.trim().is_empty() {
            return Err("workflow source value must not be blank".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct WorkflowLaunchRequest {
    pub input: WorkflowToolInput,
    /// 发起调用的会话的工作目录。启动在后台 drain 任务里做，那里拿不到调用方
    /// 的 exec ctx，所以在工具调用时取好、随请求带过去。
    pub cwd: std::path::PathBuf,
}

#[derive(Debug)]
pub enum WorkflowLaunchAck {
    Started {
        run_id: String,
        task_id: String,
        name: String,
        script_path: Option<String>,
    },
    Validated {
        name: String,
        phases: usize,
        summary: String,
    },
    Rejected {
        code: &'static str,
        detail: String,
    },
}

pub type WorkflowLaunchEnvelope = (
    WorkflowLaunchRequest,
    tokio::sync::oneshot::Sender<WorkflowLaunchAck>,
);

pub struct WorkflowLaunchHandle(pub tokio::sync::mpsc::UnboundedSender<WorkflowLaunchEnvelope>);

impl std::fmt::Debug for WorkflowLaunchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowLaunchHandle").finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowToolOutput {
    pub run_id: String,
    pub task_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_path: Option<String>,
    pub message: String,
}

pub fn render_ack(ack: WorkflowLaunchAck) -> Result<WorkflowToolOutput, (&'static str, String)> {
    match ack {
        WorkflowLaunchAck::Started {
            run_id,
            task_id,
            name,
            script_path,
        } => {
            let iterate = script_path
                .as_deref()
                .map(|p| {
                    format!(
                        " 可编辑脚本在 {p}。改完后用该 script_path 再开一轮；同进程暂停恢复只继续这次的原始脚本。"
                    )
                })
                .unwrap_or_default();
            Ok(WorkflowToolOutput {
                message: format!(
                    "工作流 '{name}' 已在后台启动。进度看 /workflow runs，完成后会自动汇报。                     '{name}' 是给用户和 /workflow 管理用的显示名；结构化 run id 保持内部。{iterate}"
                ),
                run_id,
                task_id,
                name,
                script_path,
            })
        }
        WorkflowLaunchAck::Validated {
            name,
            phases,
            summary,
        } => Ok(WorkflowToolOutput {
            message: format!(
                "工作流 '{name}' 冒烟检查通过（声明 {phases} 个阶段；canned-host 路径 {summary}）。                 这次没有真正启动，也没有覆盖所有分支或真实依赖。接下来可以正式跑一次。"
            ),
            run_id: String::new(),
            task_id: String::new(),
            name,
            script_path: None,
        }),
        WorkflowLaunchAck::Rejected { code, detail } => Err((code, detail)),
    }
}

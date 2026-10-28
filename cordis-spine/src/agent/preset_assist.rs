//! AI 辅助写预设：用户一两句话描述，模型起草整份预设 / 改写角色提示词 / 推荐工具。
//!
//! 走当前的 `"llm"` 服务做一次采样（同 `/remember` 改写：隔离 `"sessions"`，不开会话、
//! 不进任何会话历史），**不写盘**：结果是草稿，交给客户端的编辑器让用户审过再保存。
//! 模型给的东西一律当不可信输入：工具名只留目录里有的、图标只留调用方给的库里有的、
//! 子代理 id 照 `agents/<id>.yml` 的规矩校验，其余丢掉。

use cordis::Context;
use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{LlmOutput, LogEvent, PromptRequest};
use serde::Serialize;
use serde_json::Value;

use super::presets::valid_agent_type_id;
use crate::llm::sampler::Llm;
use crate::names::LLM;

/// 目录里的一个工具：名字 + 一句简介（给模型挑）。
#[derive(Clone, Debug)]
pub struct ToolChoice {
    pub name: String,
    pub summary: String,
}

/// 模型挑的一个工具和理由。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolPick {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub persona: String,
    /// `None` = 全部工具（模型没挑出目录里有的）。
    pub tools: Option<Vec<String>>,
    pub replace_prompt: bool,
    pub listings: bool,
}

/// 起草结果（字段同网关 `preset/get`，另带挑工具的理由）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetDraft {
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub persona: String,
    pub replace_prompt: bool,
    /// `None` = 全部工具（模型没挑出目录里有的）。
    pub tools: Option<Vec<String>>,
    pub tool_reasons: Vec<ToolPick>,
    pub agents: Vec<DraftAgent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rewrite {
    /// 润色：意思和结构不变，改清楚、去重复。
    Polish,
    /// 扩写：补上职责、做事方式、输出格式、边界。
    Expand,
}

const MAX_DESCRIPTION: usize = 2_000;
const MAX_PERSONA: usize = 16 * 1024;
const MAX_AGENTS: usize = 3;

const DRAFT_SYSTEM: &str = r#"你帮用户起草一个 Dock Agent 预设。Dock 是本地编码 / 研究 Agent，预设 = 角色提示词 + 工具允许名单 + 可选的子代理。

只输出一个 JSON 对象，不要任何解释、不要代码块围栏。字段：
{
  "name": "预设名，2–12 个字",
  "description": "一句话说明它擅长什么，不超过 40 字",
  "icon": "从给定图标名里选一个，没有合适的写 null",
  "persona": "角色提示词，中文 Markdown，150–600 字：身份、做事方式、输出格式、边界（不许做什么）",
  "tools": [{"name": "工具名，必须来自给定目录", "reason": "为什么要它，不超过 20 字"}],
  "subagents": [{"id": "小写字母数字和 -，如 code-reviewer", "name": "显示名", "description": "一句话职责", "persona": "它的提示词", "tools": ["工具名"]}]
}

规则：
- 工具只挑完成这类任务真正需要的，宁少勿多；用户说只读 / 不许改文件时，不要写文件、改文件、跑命令的工具。
- 需要把活分给子代理时才配 subagents（通常 0 个，最多 2 个），并且主 Agent 要有 task 工具。
- persona 里不要复述工具的参数格式，不要编造用户没说的事实。"#;

const TOOLS_SYSTEM: &str = r#"你帮用户给一个 Dock Agent 预设挑工具。只输出 JSON：{"tools": [{"name": "工具名", "reason": "为什么要它，不超过 20 字"}]}。
工具只能来自给定目录，只挑完成这类任务真正需要的，宁少勿多；只读类任务不要写文件、改文件、跑命令的工具。不要任何解释、不要代码块围栏。"#;

const POLISH_SYSTEM: &str = "你是提示词编辑。润色用户给的 Agent 角色提示词：意思和结构保持不变，把话说清楚、去掉重复、统一口吻，保留 Markdown 标题和列表。只输出改好的提示词正文，不要解释、不要代码块围栏。";

const EXPAND_SYSTEM: &str = "你是提示词编辑。扩写用户给的 Agent 角色提示词：保留原有意思，补齐身份、做事方式、输出格式、边界（不许做什么）这几块，用中文 Markdown，控制在 600 字以内；不要编造具体事实，不要复述工具参数。只输出扩写后的提示词正文，不要解释、不要代码块围栏。";

/// 起草整份预设。`icons` 是客户端的图标库（模型只能从里面选）。
pub async fn draft_preset(
    ctx: &Context,
    description: &str,
    catalog: &[ToolChoice],
    icons: &[String],
) -> Result<PresetDraft, String> {
    let description = checked_description(description)?;
    let user = format!(
        "用户的描述：\n{description}\n\n可选图标：{}\n\n工具目录：\n{}",
        if icons.is_empty() {
            "（无，icon 写 null）".to_string()
        } else {
            icons.join("、")
        },
        catalog_lines(catalog)
    );
    let raw = ask(ctx, DRAFT_SYSTEM, user).await?;
    parse_draft(&raw, catalog, icons)
}

const MAX_QUESTION: usize = 4_000;
/// 侧聊带回去的轮数上限（问 + 答各算一条）：更早的丢掉，草稿每次都现带。
const MAX_TURNS: usize = 20;
const MAX_DRAFT_JSON: usize = 32 * 1024;

const ASK_SYSTEM: &str = r#"你是 Dock 的预设顾问，用户正在自己动手配一个 Agent 预设，边配边问你。
Dock 是本地编码 / 研究 Agent；预设 = 基本信息（名称、说明、图标）+ 角色提示词 + 工具允许名单 + 可选的子代理名册。
你只回答问题、给参考，不替用户做决定，也改不了任何字段——用户看完自己去填。
- 回答围绕用户当前所在的那一步，短而具体；必要时说清取舍。
- 要给可以直接抄的内容（提示词段落、工具名单、子代理设定）时放进代码块，方便复制。
- 提到工具只用下面目录里有的名字；目录里没有的就说没有，不要编。
- 不确定的地方直说不确定。用中文回答。"#;

/// 侧聊里的一条历史。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskTurn {
    User(String),
    Assistant(String),
}

/// 预设编辑器旁的侧聊：用户边配边问，只回参考答案，**不写盘、不改草稿**。
/// 无状态：客户端每次带上之前的问答（`history`）、当前草稿（`draft`，编辑器里那份
/// JSON）和所在步骤（`step`：basic / prompt / tools / agents）。
pub async fn ask_preset(
    ctx: &Context,
    question: &str,
    history: &[AskTurn],
    draft: Option<&Value>,
    step: Option<&str>,
    catalog: &[ToolChoice],
) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("问题是空的".into());
    }
    if question.chars().count() > MAX_QUESTION {
        return Err("问题太长了（超过 4000 字）".into());
    }
    let step = match step.map(str::trim).filter(|s| !s.is_empty()) {
        None => "（未指明）",
        Some("basic") => "基本信息",
        Some("prompt") => "角色提示词",
        Some("tools") => "工具配置",
        Some("agents") => "子代理名册",
        Some(other) => {
            return Err(format!(
                "step 只能是 basic / prompt / tools / agents：{other}"
            ))
        }
    };
    let draft = match draft {
        Some(v) if !v.is_null() => {
            let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
            if text.len() > MAX_DRAFT_JSON {
                return Err("草稿太大了（超过 32KB）".into());
            }
            text
        }
        _ => "（还没填）".into(),
    };
    // 围栏比草稿里最长的一串反引号还长一个：persona 里写了 ``` 也闭合不了这个代码块。
    let fence = fence_for(&draft);
    let system = format!(
        "{ASK_SYSTEM}\n\n用户现在在「{step}」这一步。\n\n当前草稿（用户正在编辑，未保存）：\n{fence}json\n{draft}\n{fence}\n\n工具目录：\n{}",
        catalog_lines(catalog)
    );
    let skip = history.len().saturating_sub(MAX_TURNS);
    let mut events: Vec<LogEvent> = history[skip..]
        .iter()
        .filter_map(|turn| match turn {
            AskTurn::User(t) if !t.trim().is_empty() => Some(LogEvent::User(clip(t, MAX_QUESTION))),
            AskTurn::Assistant(t) if !t.trim().is_empty() => Some(LogEvent::LlmStream(LlmOutput {
                text: clip(t, MAX_PERSONA),
                ..LlmOutput::default()
            })),
            _ => None,
        })
        .collect();
    events.push(LogEvent::User(question.to_string()));
    let out = ask_history(ctx, &system, events).await?;
    if out.trim().is_empty() {
        return Err("模型没回答，再问一次".into());
    }
    Ok(out.trim().to_string())
}

/// 按描述（和已写的提示词）推荐工具。
pub async fn suggest_tools(
    ctx: &Context,
    description: &str,
    persona: &str,
    catalog: &[ToolChoice],
) -> Result<Vec<ToolPick>, String> {
    let description = checked_description(description)?;
    let persona = persona.trim();
    let user = format!(
        "用户的描述：\n{description}\n\n{}工具目录：\n{}",
        if persona.is_empty() {
            String::new()
        } else {
            format!("已写的角色提示词：\n{}\n\n", clip(persona, MAX_PERSONA))
        },
        catalog_lines(catalog)
    );
    let raw = ask(ctx, TOOLS_SYSTEM, user).await?;
    let value = json_object(&raw)?;
    let picks = tool_picks(value.get("tools"), catalog);
    if picks.is_empty() {
        return Err("模型没从目录里挑出工具，换个说法再试".into());
    }
    Ok(picks)
}

/// 改写角色提示词（润色 / 扩写）。`description` 是预设的一句话说明，给模型当背景。
pub async fn rewrite_persona(
    ctx: &Context,
    persona: &str,
    mode: Rewrite,
    description: &str,
) -> Result<String, String> {
    let persona = persona.trim();
    if persona.is_empty() {
        return Err("角色提示词是空的，先写几句再润色 / 扩写".into());
    }
    if persona.len() > MAX_PERSONA {
        return Err("角色提示词太长了（超过 16KB）".into());
    }
    let description = description.trim();
    let user = if description.is_empty() {
        persona.to_string()
    } else {
        format!("这个 Agent 的用途：{description}\n\n要改写的提示词：\n{persona}")
    };
    let system = match mode {
        Rewrite::Polish => POLISH_SYSTEM,
        Rewrite::Expand => EXPAND_SYSTEM,
    };
    let out = strip_fence(&ask(ctx, system, user).await?);
    if out.trim().is_empty() {
        return Err("模型没回改写结果，再试一次".into());
    }
    Ok(out.trim().to_string())
}

async fn ask(ctx: &Context, system: &str, user: String) -> Result<String, String> {
    ask_history(ctx, system, vec![LogEvent::User(user)]).await
}

async fn ask_history(
    ctx: &Context,
    system: &str,
    history: Vec<LogEvent>,
) -> Result<String, String> {
    let llm = ctx
        .get::<Llm>(LLM)
        .ok_or_else(|| "模型服务没有挂载".to_string())?;
    // 同 `/remember` 改写：隔离掉 `"sessions"`，这次采样不进任何会话。
    let iso = ctx.isolate("sessions");
    let output = llm
        .stream_observed(
            &iso,
            PromptRequest {
                system: system.to_string(),
                history,
                tools: Vec::new(),
            },
            |_delta: &StreamDelta| {},
        )
        .await;
    if let Some(error) = output.error {
        return Err(format!("模型调用失败：{error}"));
    }
    Ok(output.text)
}

fn checked_description(description: &str) -> Result<&str, String> {
    let description = description.trim();
    if description.is_empty() {
        return Err("先用一两句话描述你想要的 Agent".into());
    }
    if description.chars().count() > MAX_DESCRIPTION {
        return Err("描述太长了，控制在 2000 字以内".into());
    }
    Ok(description)
}

fn catalog_lines(catalog: &[ToolChoice]) -> String {
    catalog
        .iter()
        .map(|t| format!("- {}: {}", t.name, t.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 模型输出 → 校验过的草稿。纯函数，测试直接喂。
pub fn parse_draft(
    raw: &str,
    catalog: &[ToolChoice],
    icons: &[String],
) -> Result<PresetDraft, String> {
    let v = json_object(raw)?;
    let text = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let persona = strip_fence(&text(&v, "persona")).trim().to_string();
    if persona.is_empty() {
        return Err("模型没写出角色提示词，换个说法再试".into());
    }
    let name = clip(&text(&v, "name"), 40);
    let icon = v
        .get("icon")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|i| icons.iter().any(|x| x == i))
        .map(String::from);
    let picks = tool_picks(v.get("tools"), catalog);
    let tools = (!picks.is_empty()).then(|| picks.iter().map(|p| p.name.clone()).collect());

    let mut agents: Vec<DraftAgent> = Vec::new();
    for a in v
        .get("subagents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if agents.len() >= MAX_AGENTS {
            break;
        }
        let id = text(a, "id").to_lowercase();
        let persona = text(a, "persona");
        if !valid_agent_type_id(&id) || persona.is_empty() || agents.iter().any(|x| x.id == id) {
            continue;
        }
        let names: Vec<String> = a
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|n| catalog.iter().any(|t| t.name == *n))
            .map(String::from)
            .fold(Vec::new(), |mut acc, n| {
                if !acc.contains(&n) {
                    acc.push(n);
                }
                acc
            });
        let name = clip(&text(a, "name"), 40);
        agents.push(DraftAgent {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            description: clip(&text(a, "description"), 120),
            persona,
            tools: (!names.is_empty()).then_some(names),
            replace_prompt: false,
            listings: false,
        });
    }

    Ok(PresetDraft {
        name: if name.is_empty() {
            "新预设".into()
        } else {
            name
        },
        description: clip(&text(&v, "description"), 120),
        icon,
        persona,
        replace_prompt: false,
        tools,
        tool_reasons: picks,
        agents,
    })
}

/// `[{name, reason}]`（或纯字符串数组）→ 目录里有的、去重后的挑选。
fn tool_picks(v: Option<&Value>, catalog: &[ToolChoice]) -> Vec<ToolPick> {
    let mut out: Vec<ToolPick> = Vec::new();
    for item in v.and_then(Value::as_array).into_iter().flatten() {
        let (name, reason) = match item {
            Value::String(s) => (s.trim().to_string(), String::new()),
            Value::Object(_) => (
                item.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                item.get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            ),
            _ => continue,
        };
        if catalog.iter().any(|t| t.name == name) && !out.iter().any(|p| p.name == name) {
            out.push(ToolPick {
                name,
                reason: clip(&reason, 60),
            });
        }
    }
    out
}

/// 从模型输出里抠出 JSON 对象：容忍代码块围栏和前后多余的话。
fn json_object(raw: &str) -> Result<Value, String> {
    let bad = || "模型没回可用的 JSON，再试一次".to_string();
    let start = raw.find('{').ok_or_else(bad)?;
    let end = raw.rfind('}').ok_or_else(bad)?;
    if end < start {
        return Err(bad());
    }
    let v: Value = serde_json::from_str(&raw[start..=end]).map_err(|_| bad())?;
    if v.is_object() {
        Ok(v)
    } else {
        Err(bad())
    }
}

/// 去掉整段包着的 ``` 围栏（模型常这么回）。
fn strip_fence(text: &str) -> String {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t.to_string();
    };
    let body = rest
        .split_once('\n')
        .map(|(_, b)| b)
        .unwrap_or("")
        .trim_end();
    body.strip_suffix("```")
        .unwrap_or(body)
        .trim_end()
        .to_string()
}

fn clip(s: &str, max_chars: usize) -> String {
    s.trim().chars().take(max_chars).collect()
}

/// 包住 `text` 用的代码围栏：至少三个反引号，比正文里最长的一串反引号多一个。
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 草稿里带 ``` 也闭合不了包它的围栏。
    #[test]
    fn the_draft_fence_outlasts_backticks_in_the_draft() {
        assert_eq!(fence_for("{}"), "```");
        assert_eq!(fence_for("persona: 用 ``` 包代码"), "````");
        assert_eq!(fence_for("`````"), "``````");
    }
    use crate::agent::runtime::BoxFuture;
    use crate::llm::sampler::Sampler;
    use std::sync::{Arc, Mutex};

    /// 记下发出去的请求，回一句固定答案。
    struct Recording {
        seen: Arc<Mutex<Option<PromptRequest>>>,
    }

    impl Sampler for Recording {
        fn sample<'a>(
            &'a self,
            request: PromptRequest,
            _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            *self.seen.lock().unwrap() = Some(request);
            Box::pin(async {
                LlmOutput {
                    text: "  只读审查要 read_file 和 grep。  ".into(),
                    ..LlmOutput::default()
                }
            })
        }
    }

    /// 侧聊：之前的问答按角色回放、草稿和所在步骤进系统提示、不带工具，答案原样回。
    #[tokio::test]
    async fn ask_preset_replays_history_and_carries_draft_and_step() {
        let root = Context::new();
        let seen = Arc::new(Mutex::new(None));
        root.provide(
            LLM,
            Llm::from_sampler(root.clone(), Arc::new(Recording { seen: seen.clone() })),
        )
        .unwrap();
        let history = [
            AskTurn::User("我在做代码审查员".into()),
            AskTurn::Assistant("好的".into()),
        ];
        let draft = serde_json::json!({ "name": "审查员", "tools": ["read_file"] });
        let answer = ask_preset(
            &root,
            "要哪些工具？",
            &history,
            Some(&draft),
            Some("tools"),
            &catalog(),
        )
        .await
        .unwrap();
        assert_eq!(answer, "只读审查要 read_file 和 grep。");

        let req = seen.lock().unwrap().take().unwrap();
        assert!(req.tools.is_empty(), "侧聊不给工具");
        assert!(req.system.contains("「工具配置」"), "{}", req.system);
        assert!(req.system.contains("\"审查员\""), "{}", req.system);
        assert!(req.system.contains("grep"), "工具目录要在：{}", req.system);
        assert_eq!(req.history.len(), 3, "{:?}", req.history);
        assert!(matches!(&req.history[0], LogEvent::User(t) if t == "我在做代码审查员"));
        assert!(matches!(&req.history[1], LogEvent::LlmStream(o) if o.text == "好的"));
        assert!(matches!(&req.history[2], LogEvent::User(t) if t == "要哪些工具？"));

        let bad = ask_preset(&root, "x", &[], None, Some("deploy"), &catalog()).await;
        assert!(bad.unwrap_err().contains("step"));
        let empty = ask_preset(&root, "  ", &[], None, None, &catalog()).await;
        assert!(empty.is_err());
    }

    fn catalog() -> Vec<ToolChoice> {
        [
            "read_file",
            "grep",
            "glob",
            "write_file",
            "bash",
            "task",
            "web_fetch",
        ]
        .iter()
        .map(|n| ToolChoice {
            name: (*n).into(),
            summary: format!("{n} 的简介"),
        })
        .collect()
    }

    fn icons() -> Vec<String> {
        vec!["shield".into(), "code".into()]
    }

    #[test]
    fn draft_keeps_only_known_tools_icons_and_valid_roles() {
        let raw = r#"好的，这是草稿：
```json
{"name": "代码审查员", "description": "只读审代码", "icon": "rocket-ship",
 "persona": "你是审查员。\n## 边界\n- 不改文件",
 "tools": [{"name": "read_file", "reason": "读代码"}, {"name": "grep", "reason": "搜"},
           {"name": "rm_rf", "reason": "编的"}, {"name": "read_file", "reason": "重复"}, "task"],
 "subagents": [
   {"id": "Explorer", "name": "探索", "description": "找文件", "persona": "只读", "tools": ["glob", "nope"]},
   {"id": "bad id", "persona": "x"},
   {"id": "empty", "persona": ""}
 ]}
```"#;
        let d = parse_draft(raw, &catalog(), &icons()).unwrap();
        assert_eq!(d.name, "代码审查员");
        assert_eq!(d.icon, None, "图标库外的丢掉");
        assert_eq!(
            d.tools.as_deref(),
            Some(&["read_file".to_string(), "grep".into(), "task".into()][..])
        );
        assert_eq!(d.tool_reasons[0].reason, "读代码");
        assert_eq!(d.agents.len(), 1, "{:?}", d.agents);
        assert_eq!(d.agents[0].id, "explorer");
        assert_eq!(
            d.agents[0].tools.as_deref(),
            Some(&["glob".to_string()][..])
        );
        assert!(!d.replace_prompt);
    }

    #[test]
    fn draft_without_persona_or_json_is_an_error() {
        assert!(parse_draft("我不知道", &catalog(), &icons()).is_err());
        assert!(parse_draft(r#"{"name": "x", "persona": " "}"#, &catalog(), &icons()).is_err());
        assert!(parse_draft("[1, 2]", &catalog(), &icons()).is_err());
    }

    #[test]
    fn draft_with_no_known_tools_means_all_tools_and_default_name() {
        let d = parse_draft(
            r#"{"persona": "你是助手", "icon": "shield", "tools": [{"name": "made_up"}]}"#,
            &catalog(),
            &icons(),
        )
        .unwrap();
        assert_eq!(d.tools, None);
        assert_eq!(d.name, "新预设");
        assert_eq!(d.icon.as_deref(), Some("shield"));
    }

    #[test]
    fn fences_are_stripped() {
        assert_eq!(
            strip_fence("```markdown\n# 标题\n正文\n```"),
            "# 标题\n正文"
        );
        assert_eq!(strip_fence("  普通文本  "), "普通文本");
    }
}

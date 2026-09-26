//! `preset/list`：GUI 新对话页选预设用。预设在创建会话时选定（`thread/start
//! {presetId}`），之后不在会话中途换，所以不给切换方法。
//! `preset/create` / `preset/delete`：GUI 的自定义预设（写用户层 `~/.dock/presets`）。
//! `preset/get` / `preset/update` / `tool/catalog`：GUI 预设编辑器（同 TUI `/preset` 画布）。
//! `preset/draft` / `preset/rewrite` / `preset/suggestTools`：AI 辅助起草，只回草稿、不写盘。

use serde_json::{json, Value};

use cordis_spine::{
    draft_preset, is_shipped, rewrite_persona, shipped_roles, AgentPreset, AgentPresets,
    PresetEdit, PresetOrigin, Rewrite, SubagentDef, ToolChoice, Tools, AGENT_PRESETS, TOOLS,
};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;

/// 顺序和 TUI `/preset` 一样。`defaultId` 是不带 `presetId` 开新会话时用的那个。
/// 坏掉的预设也列出来（`available: false` + `error`），但 `thread/start` 会拒。
pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let presets = gateway
        .ctx()
        .get::<AgentPresets>(AGENT_PRESETS)
        .ok_or_else(|| RpcError::app("unavailable", "预设服务没有挂载"))?;
    let items: Vec<Value> = presets.list().into_iter().map(summary).collect();
    Ok(json!({ "presets": items, "defaultId": presets.current_id() }))
}

/// `preset/create { name, icon?, description?, basedOn? }`：新建用户层预设，不切当前
/// 预设。`basedOn` 给了就照它复制人设、工具名单和子代理。回新预设（同 `preset/list` 的一项）。
pub fn create(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let presets = service(gateway)?;
    let text = |key: &str| params.get(key).and_then(Value::as_str);
    let name = text("name").ok_or_else(|| RpcError::invalid_params("name is required"))?;
    let made = presets
        .create_custom(
            name,
            text("icon").filter(|s| !s.is_empty()).map(String::from),
            text("description").unwrap_or(""),
            text("basedOn").filter(|s| !s.is_empty()),
        )
        .map_err(|e| RpcError::app("invalid_params", e))?;
    Ok(json!({ "preset": summary(made) }))
}

/// `preset/delete { id }`：删用户 / 项目层预设；内置的删不掉。已经用它开过的会话不受
/// 影响（会话记着预设 id，列表里找不到时客户端自己归类）。
pub fn delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("id is required"))?;
    service(gateway)?
        .delete(id)
        .map_err(|e| RpcError::app("invalid_params", e))?;
    Ok(json!({ "ok": true, "id": id }))
}

/// `preset/get { id }`：一个预设的完整定义，给编辑器用。损坏的预设 `available: false`，
/// `error` 是解析错误，`path` 是它的文件（客户端可以让用户去外部编辑器改）。
pub fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = id_param(&params)?;
    let presets = service(gateway)?;
    let p = presets
        .get(id)
        .ok_or_else(|| RpcError::app("not_found", format!("没有预设 {id}")))?;
    let path = presets.file_of(id).map(|p| p.display().to_string());
    let shipped_roles = shipped_roles(id);
    let agents: Vec<Value> = p
        .agents
        .iter()
        .map(|(role, def)| {
            json!({
                "id": role,
                "name": def.name,
                "description": def.description,
                "persona": def.persona,
                "tools": def.tools,
                "replacePrompt": def.replace_prompt,
                "listings": def.listings,
                "readOnly": def.read_only,
                "builtin": shipped_roles.iter().any(|r| r == role),
            })
        })
        .collect();
    let mut item = summary(p.clone());
    let obj = item.as_object_mut().expect("summary is an object");
    obj.insert("name".into(), json!(p.name));
    obj.insert("order".into(), json!(p.order));
    obj.insert("persona".into(), json!(p.persona));
    obj.insert("replacePrompt".into(), json!(p.replace_prompt));
    obj.insert("tools".into(), json!(p.tools));
    obj.insert("agents".into(), json!(agents));
    obj.insert("path".into(), json!(path));
    Ok(json!({ "preset": item }))
}

/// `preset/update { id, preset }`：整份写下（`preset` 同 `preset/get` 的字段：`name`
/// `description` `icon` `order` `persona` `replacePrompt` `tools` `agents[]`）。内置预设
/// 写成用户层覆盖；损坏的预设整份重写。回 `preset`（同 `preset/list` 的一项）。
pub fn update(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = id_param(&params)?;
    let body = params
        .get("preset")
        .filter(|v| v.is_object())
        .ok_or_else(|| RpcError::invalid_params("preset is required"))?;
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let flag = |v: &Value, key: &str| v.get(key).and_then(Value::as_bool).unwrap_or(false);
    let tools = |v: &Value| -> Result<Option<Vec<String>>, RpcError> {
        match v.get("tools") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => Ok(Some(
                items
                    .iter()
                    .map(|t| {
                        t.as_str()
                            .map(String::from)
                            .ok_or_else(|| RpcError::invalid_params("tools 只收字符串"))
                    })
                    .collect::<Result<_, _>>()?,
            )),
            Some(_) => Err(RpcError::invalid_params("tools 是数组或 null")),
        }
    };
    let mut agents: Vec<(String, SubagentDef)> = Vec::new();
    for a in body
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = text(a, "id");
        if agents.iter().any(|(r, _)| *r == role) {
            return Err(RpcError::app(
                "invalid_params",
                format!("子代理 id 重复：{role}"),
            ));
        }
        let def = SubagentDef {
            name: text(a, "name"),
            description: text(a, "description"),
            persona: text(a, "persona"),
            tools: tools(a)?,
            replace_prompt: flag(a, "replacePrompt"),
            listings: flag(a, "listings"),
            read_only: flag(a, "readOnly"),
        };
        agents.push((role, def));
    }
    let edit = PresetEdit {
        name: text(body, "name"),
        description: text(body, "description"),
        icon: body
            .get("icon")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        order: body.get("order").and_then(Value::as_i64),
        persona: text(body, "persona"),
        replace_prompt: flag(body, "replacePrompt"),
        tools: tools(body)?,
        agents,
    };
    let saved = service(gateway)?
        .update(id, edit)
        .map_err(|e| RpcError::app("invalid_params", e))?;
    Ok(json!({ "preset": summary(saved) }))
}

/// `tool/catalog`：预设能选的工具（同 TUI `/preset` 画布左栏：已注册的工具，不含 MCP）。
/// `summary` 是描述的第一句；`kind`：`resident` 常驻 / `deferred` 按需 / `dynamic`
/// 运行中的动态包（不受允许名单限制）。
pub fn tool_catalog(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let tools = gateway
        .ctx()
        .get::<Tools>(TOOLS)
        .ok_or_else(|| RpcError::app("unavailable", "工具表没有挂载"))?;
    let items: Vec<Value> = tools
        .specs()
        .into_iter()
        .filter(|s| !tools.is_mcp(&s.name))
        .map(|s| {
            let kind = if tools.is_dynamic(&s.name) {
                "dynamic"
            } else if tools.is_deferred(&s.name) {
                "deferred"
            } else {
                "resident"
            };
            json!({ "name": s.name, "summary": first_sentence(&s.description), "kind": kind })
        })
        .collect();
    Ok(json!({ "tools": items }))
}

/// `preset/draft { description, icons? }`：按一两句描述起草整份预设（用默认模型采样一次）。
/// 回 `draft`：`name` / `description` / `icon` / `persona` / `replacePrompt` / `tools`
/// （`null` = 全部工具）/ `toolReasons[]` / `agents[]`。**不写盘**；工具只留目录里有的，
/// 图标只留 `icons`（客户端的图标库）里有的。模型没配好或回得不对是 `draft_failed`。
pub async fn draft(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let description = params
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let icons: Vec<String> = params
        .get("icons")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    let catalog = choices(gateway)?;
    let draft = draft_preset(gateway.ctx(), description, &catalog, &icons)
        .await
        .map_err(|e| RpcError::app("draft_failed", e))?;
    Ok(json!({ "draft": draft }))
}

/// `preset/rewrite { persona, mode: "polish" | "expand", description? }`：润色 / 扩写角色
/// 提示词，回 `persona`。不写盘。
pub async fn rewrite(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or("");
    let mode = match text("mode") {
        "polish" => Rewrite::Polish,
        "expand" => Rewrite::Expand,
        other => {
            return Err(RpcError::invalid_params(format!(
                "mode 只能是 polish / expand：{other}"
            )))
        }
    };
    let persona = rewrite_persona(gateway.ctx(), text("persona"), mode, text("description"))
        .await
        .map_err(|e| RpcError::app("draft_failed", e))?;
    Ok(json!({ "persona": persona }))
}

/// `preset/suggestTools { description, persona? }`：按描述推荐工具，回 `tools[]`
/// （`name` / `reason`，都在 `tool/catalog` 里）。不写盘。
pub async fn suggest_tools(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or("");
    let catalog = choices(gateway)?;
    let picks = cordis_spine::suggest_tools(
        gateway.ctx(),
        text("description"),
        text("persona"),
        &catalog,
    )
    .await
    .map_err(|e| RpcError::app("draft_failed", e))?;
    Ok(json!({ "tools": picks }))
}

/// 给模型挑的工具目录：同 `tool/catalog`（不含 MCP）。
fn choices(gateway: &GatewayHandle) -> Result<Vec<ToolChoice>, RpcError> {
    let tools = gateway
        .ctx()
        .get::<Tools>(TOOLS)
        .ok_or_else(|| RpcError::app("unavailable", "工具表没有挂载"))?;
    Ok(tools
        .specs()
        .into_iter()
        .filter(|s| !tools.is_mcp(&s.name))
        .map(|s| ToolChoice {
            summary: first_sentence(&s.description),
            name: s.name,
        })
        .collect())
}

/// 描述的第一句（到第一个句号 / 换行），最多 80 个字符。
fn first_sentence(description: &str) -> String {
    let line = description.trim().lines().next().unwrap_or("").trim();
    let mut end = line.len();
    for pat in ["。", ". ", "；"] {
        if let Some(i) = line.find(pat) {
            end = end.min(i + if pat == ". " { 1 } else { pat.len() });
        }
    }
    let cut = &line[..end];
    if cut.chars().count() > 80 {
        let mut s: String = cut.chars().take(79).collect();
        s.push('…');
        s
    } else {
        cut.to_string()
    }
}

fn id_param(params: &Value) -> Result<&str, RpcError> {
    params
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("id is required"))
}

fn service(gateway: &GatewayHandle) -> Result<std::sync::Arc<AgentPresets>, RpcError> {
    gateway
        .ctx()
        .get::<AgentPresets>(AGENT_PRESETS)
        .ok_or_else(|| RpcError::app("unavailable", "预设服务没有挂载"))
}

fn summary(p: AgentPreset) -> Value {
    json!({
        "id": p.id,
        "label": if p.name.trim().is_empty() { p.id.clone() } else { p.name },
        "description": p.description,
        "icon": p.icon,
        // 内置预设的 id（`origin` 可能是 user / project：用户改过的覆盖层）。删它 = 丢掉
        // 改动、恢复内置版本，客户端要照这个说清楚。
        "builtin": is_shipped(&p.id),
        "origin": match p.origin {
            PresetOrigin::Shipped => "shipped",
            PresetOrigin::User => "user",
            PresetOrigin::Project => "project",
        },
        "available": p.broken.is_none(),
        "error": p.broken,
    })
}

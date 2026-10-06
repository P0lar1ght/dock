//! 桌面 GUI 设置页：改 `~/.dock/config.toml`、管 MCP / CUA / 插件 / 密钥 / 配对 / 设备。
//!
//! 这里的方法**只给受信连接**（`dock serve` 交给父进程的 ticket）：能改 MCP 启动命令
//! 就等于能在本机跑任意程序，配对来的网页和远程设备一律 `forbidden`（见
//! [`is_settings_method`] 与 `ws.rs`）。读写都只碰用户级配置。

use serde_json::{json, Value};

use cordis_base::config::{self, edit};
use cordis_base::cua::Perms;
use cordis_spine::{
    AppSettings, Browser, BrowserState, Computer, ComputerState, CuaAction, DynamicRunner, Mcp,
    Memory, PersistScope, BROWSER, COMPUTER, DYNAMIC_CORDIS_RUNNER, MCP, MEMORY, SETTINGS,
};

use crate::devices;
use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};

use cordis::{plugin, Inject, Plugin};

use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("缺少 {key}")))
}

fn flag(params: &Value, key: &str) -> Result<bool, RpcError> {
    params
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params(format!("缺少 {key}（true / false）")))
}

fn write_failed(e: String) -> RpcError {
    RpcError::app("write_failed", e)
}

fn env_value(name: &str) -> Value {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Value::from(v),
        _ => Value::Null,
    }
}

// ---- 配置文件 ----

/// `config/status`：数据目录、配置文件在哪、能不能解析。
pub fn config_status() -> Result<Value, RpcError> {
    let path = edit::user_config_path();
    Ok(json!({
        "dockHome": config::dock_home().display().to_string(),
        "path": path.display().to_string(),
        "exists": path.exists(),
        "error": edit::parse_error()
    }))
}

/// `config/get`：白名单键在用户配置里的值 + 会盖掉它们的环境变量。
pub fn config_get(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let memory_forced = gateway
        .ctx()
        .get::<Memory>(MEMORY)
        .map(|m| m.config().force_disabled)
        .unwrap_or_else(|| config::load_memory_config().force_disabled);
    Ok(json!({
        "values": edit::read_settings(),
        "env": {
            "DOCK_MODEL": env_value("DOCK_MODEL"),
            "DOCK_MEMORY": env_value("DOCK_MEMORY"),
            "DOCK_BROWSER_HEADED": env_value("DOCK_BROWSER_HEADED"),
            "DOCK_CUA_DRIVER": env_value("DOCK_CUA_DRIVER")
        },
        "memoryForcedOff": memory_forced
    }))
}

/// `config/set { key, value }`：写一个白名单键（`value: null` = 删掉回默认）。
pub fn config_set(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let key = text(&params, "key")?;
    let value = params.get("value").cloned().unwrap_or(Value::Null);
    edit::write_setting(key, &value).map_err(write_failed)?;
    // 记忆的开关要重挂工具；其余键都是用到时现读。
    if key.starts_with("memory.") {
        if let Some(memory) = gateway.ctx().get::<Memory>(MEMORY) {
            memory.reload();
        }
    }
    let browser_note = key == "browser.headed";
    Ok(json!({
        "ok": true,
        "values": edit::read_settings(),
        // 已开的 Chromium 不重启：下次拉起才按新设置。
        "appliesNextLaunch": browser_note
    }))
}

/// `config/env { names }`：这些环境变量在 Dock 进程里设了没有（只回有没有，不回值）。
/// 编辑模型时「变量名」旁边的「已检测到」用。
pub fn config_env(params: Value) -> Result<Value, RpcError> {
    let names = params
        .get("names")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid_params("缺少 names"))?;
    let mut out = serde_json::Map::new();
    for name in names.iter().filter_map(Value::as_str).take(32) {
        let set = std::env::var(name).is_ok_and(|v| !v.trim().is_empty());
        out.insert(name.to_string(), Value::Bool(set));
    }
    Ok(Value::Object(out))
}

// ---- 模型 ----

fn settings(gateway: &GatewayHandle) -> Result<std::sync::Arc<AppSettings>, RpcError> {
    gateway
        .ctx()
        .get::<AppSettings>(SETTINGS)
        .ok_or_else(|| RpcError::app("unavailable", "settings 服务没有挂载"))
}

/// 目录里除了 `except` 以外的 id（查重用）。
fn other_model_ids(except: Option<&str>) -> Vec<String> {
    config::load_catalog()
        .into_iter()
        .map(|m| m.id)
        .chain(edit::user_model_ids())
        .filter(|id| Some(id.as_str()) != except)
        .collect()
}

/// `model/get { id }`：用户配置里这条模型的全部字段（不含行内密钥本身）。
pub fn model_get(params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "id")?;
    let entry = edit::read_model(id).ok_or_else(|| {
        RpcError::app(
            "not_editable",
            format!("模型 {id} 不在 ~/.dock/config.toml 里（可能写在项目配置里），这里改不了"),
        )
    })?;
    Ok(json!({ "model": entry }))
}

/// 新的默认模型也要让**之后新开的页**用上：新页从根页的设置抄一份。
/// `DOCK_MODEL` 设着时它说了算，不动。
fn apply_default(gateway: &GatewayHandle, id: &str) {
    if std::env::var("DOCK_MODEL").is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }
    if let Ok(settings) = settings(gateway) {
        if settings.model() != id {
            settings.set_model(id);
        }
    }
}

/// `model/save { model, originalId?, makeDefault? }`：新建或保存。改名时带 `originalId`。
pub fn model_save(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let entry: edit::ModelEntry = serde_json::from_value(
        params
            .get("model")
            .cloned()
            .ok_or_else(|| RpcError::invalid_params("缺少 model"))?,
    )
    .map_err(|e| RpcError::invalid_params(format!("model 字段不对：{e}")))?;
    let original = params
        .get("originalId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let make_default = params
        .get("makeDefault")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let others = other_model_ids(original);
    edit::save_model(&entry, original, make_default, &others)
        .map_err(|e| RpcError::app("invalid_model", e))?;
    let id = entry.id.trim();
    let is_default = config::load_default_model().as_deref() == Some(id);
    if make_default || is_default {
        apply_default(gateway, id);
    }
    // 根页正在用改名前的那条：跟着改名，不然它指向一个已经不存在的 id。
    if let (Some(old), Ok(s)) = (original, settings(gateway)) {
        if old != id && s.model() == old {
            s.set_model(id);
        }
    }
    crate::handlers::connection::model_list(gateway)
}

/// `model/delete { id }`：删掉；删的是默认模型时默认改成目录里的下一条。
pub fn model_delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "id")?;
    let next = config::load_catalog()
        .into_iter()
        .map(|m| m.id)
        .find(|m| m != id);
    edit::delete_model(id, next.as_deref()).map_err(write_failed)?;
    if let Some(default) = config::load_default_model() {
        apply_default(gateway, &default);
    }
    crate::handlers::connection::model_list(gateway)
}

/// `model/default { id }`：设成 `[models].default`，之后新开的对话用它。
pub fn model_default(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "id")?;
    if !config::load_catalog().iter().any(|m| m.id == id) {
        return Err(RpcError::invalid_params(format!("目录里没有模型 {id}")));
    }
    edit::set_default_model(id).map_err(write_failed)?;
    apply_default(gateway, id);
    crate::handlers::connection::model_list(gateway)
}

/// `model/test { model? , id? }`：发一次最小请求。`model` 是表单（可以没保存），
/// 只给 `id` 就测配置里那条。回 `{ ok, ms }` 或 `{ ok: false, error }`。
pub async fn model_test(params: Value) -> Result<Value, RpcError> {
    let choice = match params.get("model") {
        Some(raw) => {
            let entry: edit::ModelEntry = serde_json::from_value(raw.clone())
                .map_err(|e| RpcError::invalid_params(format!("model 字段不对：{e}")))?;
            let existing = params
                .get("originalId")
                .and_then(Value::as_str)
                .and_then(edit::inline_key);
            edit::choice_from_entry(&entry, existing)
                .map_err(|e| RpcError::app("invalid_model", e))?
        }
        None => {
            let id = text(&params, "id")?;
            config::lookup_model(id)
                .ok_or_else(|| RpcError::invalid_params(format!("目录里没有模型 {id}")))?
        }
    };
    Ok(match cordis_spine::probe_model(&choice).await {
        Ok(ms) => json!({ "ok": true, "ms": ms, "backend": choice.default_backend().name() }),
        Err(error) => {
            json!({ "ok": false, "error": error, "backend": choice.default_backend().name() })
        }
    })
}

// ---- MCP ----

fn mcp(gateway: &GatewayHandle) -> Result<std::sync::Arc<Mcp>, RpcError> {
    gateway
        .ctx()
        .get::<Mcp>(MCP)
        .ok_or_else(|| RpcError::app("unavailable", "MCP 服务没有挂载"))
}

/// `mcp/get { name }`：用户配置里这台服务器的完整行（编辑表单用）。
pub fn mcp_get(params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    let entry = edit::read_mcp(name).ok_or_else(|| {
        RpcError::app(
            "not_editable",
            format!("MCP 服务 {name} 不在 ~/.dock/config.toml 里，这里改不了"),
        )
    })?;
    Ok(json!({ "server": entry }))
}

/// 写完配置让 MCP 对一次账，回和 `mcp/list` 同形的列表 + 这次的摘要。
async fn reload_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let mcp = mcp(gateway)?;
    let summary = match mcp.reload().await {
        Ok(report) => report.summary(),
        Err(e) => e,
    };
    let mut list = crate::handlers::connection::mcp_list(gateway)?;
    list["summary"] = Value::from(summary);
    Ok(list)
}

/// `mcp/save { server, originalName? }`：新建或保存一台，然后重载。
pub async fn mcp_save(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let entry: edit::McpEntry = serde_json::from_value(
        params
            .get("server")
            .cloned()
            .ok_or_else(|| RpcError::invalid_params("缺少 server"))?,
    )
    .map_err(|e| RpcError::invalid_params(format!("server 字段不对：{e}")))?;
    let original = params
        .get("originalName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let others: Vec<String> = config::load_mcp_servers()
        .into_iter()
        .map(|s| s.name)
        .filter(|n| Some(n.as_str()) != original)
        .collect();
    edit::save_mcp(&entry, original, &others).map_err(|e| RpcError::app("invalid_server", e))?;
    reload_list(gateway).await
}

/// `mcp/delete { name }`：删掉用户配置里这一行（内置的删不了，只能停用）。
pub async fn mcp_delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    if edit::is_builtin_mcp(name) && edit::read_mcp(name).is_none() {
        return Err(RpcError::app(
            "builtin",
            format!("{name} 是内置服务，只能停用"),
        ));
    }
    edit::delete_mcp(name).map_err(write_failed)?;
    reload_list(gateway).await
}

/// `mcp/enable { name, enabled }`：启用 / 停用（写配置并立刻连上 / 断开）。
pub async fn mcp_enable(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    let enabled = flag(&params, "enabled")?;
    let result = mcp(gateway)?.set_server_enabled(name, enabled).await;
    let mut list = crate::handlers::connection::mcp_list(gateway)?;
    // 启用了但没连上不算这次调用失败：状态里有原因。原话放在 `error` 里给界面提示。
    if let Err(e) = result {
        list["error"] = Value::from(e);
    }
    Ok(list)
}

/// `mcp/tool/enable { server, tool, enabled }`：单个工具开关。
pub async fn mcp_tool_enable(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let server = text(&params, "server")?;
    let tool = text(&params, "tool")?;
    let enabled = flag(&params, "enabled")?;
    mcp(gateway)?
        .set_tool_enabled(server, tool, enabled)
        .await
        .map_err(write_failed)?;
    crate::handlers::connection::mcp_list(gateway)
}

/// `mcp/login { name }`：HTTP 服务器的浏览器 OAuth。开系统浏览器，等用户授权完回来。
pub async fn mcp_login(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    mcp(gateway)?
        .authenticate(name)
        .await
        .map_err(|e| RpcError::app("login_failed", e))?;
    crate::handlers::connection::mcp_list(gateway)
}

// ---- CUA ----

/// `cua/status`：cua-driver 状态机 + 授权 + 正在跑的安装 / 授权进度。
pub fn cua_status(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let computer = gateway
        .ctx()
        .get::<Computer>(COMPUTER)
        .ok_or_else(|| RpcError::app("unavailable", "computer 服务没有挂载"))?;
    let (state, detail) = match computer.state() {
        ComputerState::NotMounted => ("not_mounted", String::new()),
        ComputerState::Missing => ("missing", String::new()),
        ComputerState::NeedsPermission { .. } => ("needs_permission", String::new()),
        ComputerState::Disabled => ("disabled", String::new()),
        ComputerState::NeedsAuth { detail } => ("needs_auth", detail),
        ComputerState::Unreachable { detail } => ("unreachable", detail),
        ComputerState::Connected { detail, .. } => ("connected", detail),
    };
    let perms = match computer.perms() {
        Perms::Unknown => json!(null),
        Perms::NotRequired => json!({ "required": false }),
        Perms::Granted => {
            json!({ "required": true, "accessibility": true, "screenRecording": true })
        }
        Perms::Missing {
            accessibility,
            screen_recording,
        } => {
            json!({ "required": true, "accessibility": accessibility, "screenRecording": screen_recording })
        }
    };
    let job = computer.job_view().map(|job| {
        json!({
            "action": match job.action { CuaAction::Install => "install", CuaAction::Grant => "grant" },
            "lines": job.lines,
            "running": job.finished.is_none(),
            "ok": job.finished.as_ref().map(Result::is_ok),
            "message": job.finished.map(|r| r.unwrap_or_else(|e| e))
        })
    });
    Ok(json!({
        "state": state,
        "detail": detail,
        "driverPath": computer.driver_path().map(|p| p.display().to_string()),
        "driverEnv": env_value("DOCK_CUA_DRIVER"),
        "perms": perms,
        "job": job,
        "installSteps": cordis_base::cua::install_plan_lines()
    }))
}

/// `cua/action { action: "install" | "grant" | "refresh" }`。安装 / 授权在后台跑，
/// 进度用 `cua/status` 轮询。
pub fn cua_action(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let computer = gateway
        .ctx()
        .get::<Computer>(COMPUTER)
        .ok_or_else(|| RpcError::app("unavailable", "computer 服务没有挂载"))?;
    match text(&params, "action")? {
        "install" => computer.start(CuaAction::Install),
        "grant" => computer.start(CuaAction::Grant),
        "refresh" => {
            computer.clear_finished();
            computer.refresh();
            Ok(())
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "action 只能是 install / grant / refresh：{other}"
            )))
        }
    }
    .map_err(|e| RpcError::app("busy", e))?;
    cua_status(gateway)
}

/// `browser/status`：内置浏览器 MCP 的状态 + 有头设置。
pub fn browser_status(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let browser = gateway.ctx().get::<Browser>(BROWSER);
    let (state, detail) = match browser.as_ref().map(|b| b.state()) {
        None | Some(BrowserState::NotMounted) => ("not_mounted", String::new()),
        Some(BrowserState::Missing) => ("missing", String::new()),
        Some(BrowserState::Disabled) => ("disabled", String::new()),
        Some(BrowserState::Connected { tools }) => ("connected", format!("{tools} 个工具")),
        Some(BrowserState::Unreachable { detail }) => ("unreachable", detail),
    };
    Ok(json!({
        "state": state,
        "detail": detail,
        "headed": config::load_browser_headed(),
        "headedEnv": config::dock_browser_headed_env_override()
    }))
}

// ---- 插件 / 技能 / 密钥 ----

fn runner(gateway: &GatewayHandle) -> Result<std::sync::Arc<DynamicRunner>, RpcError> {
    gateway
        .ctx()
        .get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .ok_or_else(|| RpcError::app("unavailable", "插件运行器没有挂载"))
}

fn scope_name(scope: PersistScope) -> &'static str {
    match scope {
        PersistScope::User => "user",
        PersistScope::Project => "project",
    }
}

/// 已知的项目目录：开着的页 + 跨目录名册里的会话 + 进程 cwd（TUI 的启动目录）。
fn known_projects(gateway: &GatewayHandle) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut push = |p: std::path::PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for page in crate::threads::open_pages(gateway) {
        push(cordis_spine::session_cwd(&page.ctx));
    }
    for entry in crate::handlers::thread::fresh_roster(gateway).list() {
        push(entry.cwd);
    }
    if let Ok(cwd) = std::env::current_dir() {
        push(cwd);
    }
    out
}

/// 用户级 + 每个已知项目的插件根。
fn plugin_roots_all(gateway: &GatewayHandle) -> Vec<(PersistScope, std::path::PathBuf)> {
    let mut roots = vec![(
        PersistScope::User,
        cordis_spine::persist_root(PersistScope::User),
    )];
    for project in known_projects(gateway) {
        roots.push((PersistScope::Project, cordis_spine::project_root(&project)));
    }
    roots
}

/// 界面传来的插件目录：规范化后必须是某个已知插件根的直接子目录，否则拒
/// （删除走 `remove_dir_all`，不能让它删到别处去）。canonicalize 把 symlink
/// 解开、把 `..` 压平——连最后一个分量的符号链接一起解，否则 `<根>/link -> ~`
/// 这种能删到根外；不存在的路径直接拒。
fn plugin_dir(gateway: &GatewayHandle, params: &Value) -> Result<std::path::PathBuf, RpcError> {
    let canon = std::path::PathBuf::from(text(params, "path")?)
        .canonicalize()
        .map_err(|_| RpcError::invalid_params("不是插件目录"))?;
    let parent = canon.parent().unwrap_or(std::path::Path::new(""));
    let ok = plugin_roots_all(gateway).iter().any(|(_, root)| {
        root.canonicalize()
            .map(|root| root.as_path() == parent)
            .unwrap_or(false)
    });
    if !ok {
        return Err(RpcError::invalid_params("不是插件目录"));
    }
    // 用规范化后的全路径，后面不再依赖界面传来的文本形式。
    Ok(canon)
}

/// 定义动态插件的那个会话（按 `Sessions::identity` 找开着的页）。
fn owner_page(gateway: &GatewayHandle, identity: &str) -> Option<crate::threads::Page> {
    crate::threads::page_by_identity(gateway, identity)
}

/// `plugin/list`：永久插件（用户级 + 每个项目，按根分组）和动态插件（会话里定义的）。
pub fn plugin_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let runner = runner(gateway)?;
    let projects = known_projects(gateway);
    let roots = plugin_roots_all(gateway);
    let disk: Vec<Value> = runner
        .disk_plugins_in(&roots)
        .into_iter()
        .map(|p| {
            let project = p
                .root
                .parent()
                .and_then(std::path::Path::parent)
                .filter(|_| p.scope == PersistScope::Project)
                .map(|d| d.display().to_string());
            json!({
                "id": p.id,
                "name": p.name,
                "purpose": p.purpose,
                "scope": scope_name(p.scope),
                "project": project,
                "root": p.root.display().to_string(),
                "path": p.path.display().to_string(),
                "enabled": p.enabled,
                "loaded": p.loaded,
                "running": p.running,
                "shadowedBy": p.shadowed_by.map(|d| d.display().to_string()),
                "error": p.error
            })
        })
        .collect();
    let session: Vec<Value> = runner
        .session_plugins()
        .into_iter()
        .map(|p| {
            let page = owner_page(gateway, &p.session_id);
            json!({
                "id": p.plugin_id,
                "name": p.name,
                "purpose": p.purpose,
                "factory": p.factory,
                "running": p.running,
                "error": p.error,
                "threadId": page.as_ref().map(crate::threads::Page::session_id),
                "threadTitle": page.as_ref().and_then(|pg| pg.sessions().ok()).map(|s| s.live_title()),
                "cwd": page.as_ref().map(|pg| cordis_spine::session_cwd(&pg.ctx).display().to_string())
            })
        })
        .collect();
    Ok(json!({
        "disk": disk,
        "session": session,
        "userRoot": cordis_spine::persist_root(PersistScope::User).display().to_string(),
        "projects": projects.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
    }))
}

/// `plugin/enable { path, enabled }`：按目录启停一个永久插件（立刻装上跑 / 停掉）。
pub async fn plugin_enable(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let dir = plugin_dir(gateway, &params)?;
    let enabled = flag(&params, "enabled")?;
    runner(gateway)?
        .set_disk_enabled_at(&dir, enabled)
        .await
        .map_err(|e| RpcError::app("plugin_failed", e))?;
    plugin_list(gateway)
}

/// `plugin/delete { path }`：停掉并删这个插件目录。
pub async fn plugin_delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let dir = plugin_dir(gateway, &params)?;
    runner(gateway)?
        .delete_disk_at(&dir)
        .await
        .map_err(|e| RpcError::app("plugin_failed", e))?;
    plugin_list(gateway)
}

/// `plugin/promote { id, scope }`：把动态插件写成永久。`project` 写进定义它的那个
/// 会话的项目（`<项目>/.dock/plugins`），`user` 写进 `~/.dock/plugins`。
pub async fn plugin_promote(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "id")?;
    let runner = runner(gateway)?;
    let (scope, root) = match params.get("scope").and_then(Value::as_str) {
        None | Some("user") => (
            PersistScope::User,
            cordis_spine::persist_root(PersistScope::User),
        ),
        Some("project") => {
            let owner = runner
                .session_plugins()
                .into_iter()
                .find(|p| p.plugin_id == id)
                .ok_or_else(|| RpcError::app("plugin_failed", format!("没有叫 {id} 的动态插件")))?;
            let page = owner_page(gateway, &owner.session_id).ok_or_else(|| {
                RpcError::app(
                    "plugin_failed",
                    "定义它的会话已经关了，不知道是哪个项目；存到用户级吧",
                )
            })?;
            (
                PersistScope::Project,
                cordis_spine::project_root(&cordis_spine::session_cwd(&page.ctx)),
            )
        }
        Some(other) => {
            return Err(RpcError::invalid_params(format!(
                "scope 只能是 user / project：{other}"
            )))
        }
    };
    runner
        .promote_session(id, scope, root)
        .await
        .map_err(|e| RpcError::app("plugin_failed", e))?;
    plugin_list(gateway)
}

/// `plugin/discard { id }`：丢掉一个动态插件（停掉，从内存里删掉）。
pub async fn plugin_discard(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "id")?;
    runner(gateway)?
        .discard_session(id)
        .await
        .map_err(|e| RpcError::app("plugin_failed", e))?;
    plugin_list(gateway)
}

/// 开了某个项目的会话：装上这个项目的永久插件（后台跑，不挡开页）。
pub fn boot_project_plugins(gateway: &GatewayHandle, project: std::path::PathBuf) {
    let Some(runner) = gateway.ctx().get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
        return;
    };
    tokio::spawn(async move { runner.boot_project(&project).await });
}

/// `skill/list`：内置 / 用户级的技能列一次；每个已知项目自己那几层（`skills/`、
/// `.agents/skills/`、`.dock/skills/`）按项目列。被更高层同名遮住的标出来。
pub fn skill_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    use cordis_spine::SkillScope;
    let global = |scope: SkillScope| matches!(scope, SkillScope::Builtin | SkillScope::User);
    let mut globals: Vec<(cordis_spine::SkillInfo, Vec<String>)> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    for project in known_projects(gateway) {
        let shown = project.display().to_string();
        for (skill, shadowed) in cordis_spine::scan_all_with_shadowed(&project) {
            if global(skill.scope) {
                match globals.iter_mut().find(|(s, _)| s.path == skill.path) {
                    Some((_, by)) if shadowed => by.push(shown.clone()),
                    Some(_) => {}
                    None => globals.push((
                        skill,
                        if shadowed {
                            vec![shown.clone()]
                        } else {
                            Vec::new()
                        },
                    )),
                }
                continue;
            }
            if rows
                .iter()
                .any(|r| r["path"] == json!(skill.path.display().to_string()))
            {
                continue;
            }
            rows.push(json!({
                "name": skill.name,
                "description": skill.description,
                "scope": skill.scope.label(),
                "path": skill.path.display().to_string(),
                "userInvocable": skill.user_invocable,
                "project": shown,
                "shadowed": shadowed,
                "shadowedIn": []
            }));
        }
    }
    let mut skills: Vec<Value> = globals
        .into_iter()
        .map(|(s, by)| {
            json!({
                "name": s.name,
                "description": s.description,
                "scope": s.scope.label(),
                "path": s.path.display().to_string(),
                "userInvocable": s.user_invocable,
                "project": null,
                "shadowed": false,
                // 在这些项目里被项目自己的同名技能盖住。
                "shadowedIn": by
            })
        })
        .collect();
    skills.extend(rows);
    Ok(json!({ "skills": skills }))
}

/// `secret/list`：插件密钥的名字（不回值）。
pub fn secret_list() -> Result<Value, RpcError> {
    let names = cordis_spine::secret_names().map_err(|e| RpcError::app("read_failed", e))?;
    Ok(
        json!({ "names": names, "path": config::dock_home().join("secrets.json").display().to_string() }),
    )
}

/// `secret/set { name, value }`。
pub fn secret_set(params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    let value = params
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("缺少 value"))?;
    cordis_spine::set_secret(name, value).map_err(write_failed)?;
    secret_list()
}

/// `secret/delete { name }`。
pub fn secret_delete(params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    cordis_spine::delete_secret(name).map_err(write_failed)?;
    secret_list()
}

// ---- 配对与设备 ----

fn unix_ms(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `pairing/list`：待批的网页、已配对的来源、要不要接新的配对请求。
pub fn pairing_list(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let inner = gateway.inner();
    let mut store = inner.pairing.lock().unwrap();
    let pending: Vec<Value> = store
        .pending()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "application": p.application,
                "origin": p.origin,
                "createdAt": unix_ms(p.created_at),
                "expiresAt": unix_ms(p.expires_at)
            })
        })
        .collect();
    let bindings: Vec<Value> = store
        .bindings()
        .into_iter()
        .map(|b| json!({ "application": b.application, "origin": b.origin, "boundAt": unix_ms(b.bound_at) }))
        .collect();
    Ok(json!({
        "accepting": store.accepting(),
        "listen": gateway.listen_addr().to_string(),
        "remote": gateway.is_remote(),
        "pending": pending,
        "bindings": bindings
    }))
}

/// `pairing/resolve { id, approve }`：批准 / 拒绝一个配对请求。
pub fn pairing_resolve(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "id")?;
    let approve = flag(&params, "approve")?;
    {
        let inner = gateway.inner();
        let mut store = inner.pairing.lock().unwrap();
        if approve {
            store.confirm(id)
        } else {
            store.deny(id)
        }
        .map_err(|e| RpcError::app(e.code, e.message))?;
    }
    pairing_list(gateway)
}

/// `pairing/revoke { origin }`：撤销一个已配对的来源（它手里的 ticket 一起作废）。
pub fn pairing_revoke(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let origin = text(&params, "origin")?;
    gateway
        .inner()
        .pairing
        .lock()
        .unwrap()
        .revoke(origin)
        .map_err(|e| RpcError::app(e.code, e.message))?;
    pairing_list(gateway)
}

/// `pairing/accept { accepting }`：要不要接新的网页配对请求（已配对的不受影响）。
/// 只在本进程里生效，GUI 每次连上按自己存的偏好再设一次。
pub fn pairing_accept(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let accepting = flag(&params, "accepting")?;
    gateway
        .inner()
        .pairing
        .lock()
        .unwrap()
        .set_accepting(accepting);
    pairing_list(gateway)
}

/// `device/list`：远程设备（`dock device`）。
pub fn device_list() -> Result<Value, RpcError> {
    let list = devices::list().map_err(|e| RpcError::app("read_failed", e))?;
    let devices: Vec<Value> = list
        .into_iter()
        .map(|d| json!({ "id": d.id, "name": d.name, "createdAt": d.created_ms, "lastUsedAt": d.last_used_ms }))
        .collect();
    Ok(json!({ "devices": devices }))
}

/// `device/add { name }`：签一枚令牌。令牌**只在这次回包里出现**。
pub fn device_add(params: Value) -> Result<Value, RpcError> {
    let name = text(&params, "name")?;
    let (device, token) = devices::add(name).map_err(|e| RpcError::app("invalid_device", e))?;
    let mut list = device_list()?;
    list["device"] = json!({ "id": device.id, "name": device.name });
    list["token"] = Value::from(token);
    Ok(list)
}

/// `device/revoke { id }`（名字也认）：撤销后连着的这台几秒内被断开。
pub fn device_revoke(params: Value) -> Result<Value, RpcError> {
    let which = text(&params, "id")?;
    devices::revoke(which).map_err(|e| RpcError::app("not_found", e))?;
    device_list()
}

/// 设置页这一块：读写用户级配置、MCP、插件、密钥、配对、设备的方法。全部
/// `trusted_only`——能写 MCP 启动命令就等于能在本机跑程序，只认受信连接（`dock serve`
/// 交给桌面 GUI 的 ticket、设备令牌）。连服务器 / 跑插件 / 等浏览器登录的几条放到连接锁外跑。
pub fn gateway_settings() -> Plugin {
    plugin(
        "gateway.settings",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            const TRUSTED: MethodPolicy = MethodPolicy {
                detached: false,
                trusted_only: true,
                opens_thread: false,
            };
            const TRUSTED_DETACHED: MethodPolicy = MethodPolicy {
                detached: true,
                ..TRUSTED
            };
            register_methods(
                ctx,
                vec![
                    (
                        protocol::CONFIG_STATUS,
                        TRUSTED,
                        method(|_, _| async move { config_status() }),
                    ),
                    (
                        protocol::CONFIG_GET,
                        TRUSTED,
                        method(|gw, _| async move { config_get(&gw) }),
                    ),
                    (
                        protocol::CONFIG_SET,
                        TRUSTED,
                        method(|gw, params| async move { config_set(&gw, params) }),
                    ),
                    (
                        protocol::CONFIG_ENV,
                        TRUSTED,
                        method(|_, params| async move { config_env(params) }),
                    ),
                    (
                        protocol::MODEL_GET,
                        TRUSTED,
                        method(|_, params| async move { model_get(params) }),
                    ),
                    (
                        protocol::MODEL_SAVE,
                        TRUSTED,
                        method(|gw, params| async move { model_save(&gw, params) }),
                    ),
                    (
                        protocol::MODEL_DELETE,
                        TRUSTED,
                        method(|gw, params| async move { model_delete(&gw, params) }),
                    ),
                    (
                        protocol::MODEL_DEFAULT,
                        TRUSTED,
                        method(|gw, params| async move { model_default(&gw, params) }),
                    ),
                    (
                        protocol::MCP_GET,
                        TRUSTED,
                        method(|_, params| async move { mcp_get(params) }),
                    ),
                    (
                        protocol::MCP_TOOL_ENABLE,
                        TRUSTED,
                        method(|gw, params| async move { mcp_tool_enable(&gw, params).await }),
                    ),
                    (
                        protocol::CUA_STATUS,
                        TRUSTED,
                        method(|gw, _| async move { cua_status(&gw) }),
                    ),
                    (
                        protocol::CUA_ACTION,
                        TRUSTED,
                        method(|gw, params| async move { cua_action(&gw, params) }),
                    ),
                    (
                        protocol::BROWSER_STATUS,
                        TRUSTED,
                        method(|gw, _| async move { browser_status(&gw) }),
                    ),
                    (
                        protocol::PLUGIN_LIST,
                        TRUSTED,
                        method(|gw, _| async move { plugin_list(&gw) }),
                    ),
                    (
                        protocol::PLUGIN_DELETE,
                        TRUSTED,
                        method(|gw, params| async move { plugin_delete(&gw, params).await }),
                    ),
                    (
                        protocol::SKILL_LIST,
                        TRUSTED,
                        method(|gw, _| async move { skill_list(&gw) }),
                    ),
                    (
                        protocol::SECRET_LIST,
                        TRUSTED,
                        method(|_, _| async move { secret_list() }),
                    ),
                    (
                        protocol::SECRET_SET,
                        TRUSTED,
                        method(|_, params| async move { secret_set(params) }),
                    ),
                    (
                        protocol::SECRET_DELETE,
                        TRUSTED,
                        method(|_, params| async move { secret_delete(params) }),
                    ),
                    (
                        protocol::PAIRING_LIST,
                        TRUSTED,
                        method(|gw, _| async move { pairing_list(&gw) }),
                    ),
                    (
                        protocol::PAIRING_RESOLVE,
                        TRUSTED,
                        method(|gw, params| async move { pairing_resolve(&gw, params) }),
                    ),
                    (
                        protocol::PAIRING_REVOKE,
                        TRUSTED,
                        method(|gw, params| async move { pairing_revoke(&gw, params) }),
                    ),
                    (
                        protocol::PAIRING_ACCEPT,
                        TRUSTED,
                        method(|gw, params| async move { pairing_accept(&gw, params) }),
                    ),
                    (
                        protocol::DEVICE_LIST,
                        TRUSTED,
                        method(|_, _| async move { device_list() }),
                    ),
                    (
                        protocol::DEVICE_ADD,
                        TRUSTED,
                        method(|_, params| async move { device_add(params) }),
                    ),
                    (
                        protocol::DEVICE_REVOKE,
                        TRUSTED,
                        method(|_, params| async move { device_revoke(params) }),
                    ),
                    (
                        protocol::MODEL_TEST,
                        TRUSTED_DETACHED,
                        method(|_, params| async move { model_test(params).await }),
                    ),
                    (
                        protocol::MCP_SAVE,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { mcp_save(&gw, params).await }),
                    ),
                    (
                        protocol::MCP_DELETE,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { mcp_delete(&gw, params).await }),
                    ),
                    (
                        protocol::MCP_ENABLE,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { mcp_enable(&gw, params).await }),
                    ),
                    (
                        protocol::MCP_LOGIN,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { mcp_login(&gw, params).await }),
                    ),
                    (
                        protocol::PLUGIN_ENABLE,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { plugin_enable(&gw, params).await }),
                    ),
                    (
                        protocol::PLUGIN_PROMOTE,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { plugin_promote(&gw, params).await }),
                    ),
                    (
                        protocol::PLUGIN_DISCARD,
                        TRUSTED_DETACHED,
                        method(|gw, params| async move { plugin_discard(&gw, params).await }),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}

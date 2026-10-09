//! dock.1 protocol constants, method names, and JSON-RPC error helpers.

use serde::Serialize;
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "dock.1";
pub const LIVE_THREAD_ID: &str = "live";
pub const DEFAULT_WORKSPACE_ID: &str = "default";
pub const SERVER_NAME: &str = "dock";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const WS_PATH: &str = "/api/ws";

/// Capabilities actually implemented in this crate.
pub const CAPABILITIES: &[(&str, bool)] = &[
    ("turns", true),
    ("permissions", true),
    ("transcriptEvents", true),
    ("threadSubscriptions", true),
    ("threads", true),
    ("hostTools", false),
    ("imageInputs", true),
    ("slash", true),
    // 多线程：`thread/list {scope:"all"}` / `thread/open` / `thread/close` /
    // `thread/start {cwd}`，其它线程级方法接受任意开着的线程的会话 id。
    ("openThreads", true),
    // `preset/list`：新对话选预设（只读，预设在创建会话时定下）。
    ("presets", true),
    // `browser/view/*`：看会话正在用的浏览器标签页（CDP 画面流）并能接手操作。
    ("browserView", true),
    // `browser/view/open` 的 `url` / `viewport` + `browser/view/resize`：没标签页时替会话开页，
    // 页面视口跟着面板走。
    ("browserViewport", true),
    // `browser/view/tabs`（会话的标签页列表推送）+ `browser/view/tab`（切换 / 新开 / 关闭）：
    // 面板的标签栏。和 agent 共用当前页。
    ("browserTabs", true),
    // `desktop/view/*`：桌面面板的实时画面（agent 操作的窗口，经 cua-driver 只截图）。
    ("desktopView", true),
    // `desktop/view/cursor` / `desktop/view/status`：agent 光标（画在实时画面上）和画面状态
    // （没有窗口、截图失败）。
    ("desktopCursor", true),
    // `fs/list` / `fs/read` / `fs/find`：只读看会话工作区里的文件（文件面板）。
    ("workspaceFiles", true),
    // `fs/dirs { path?, hidden? }`：列任意绝对路径下的子目录（远程 GUI 选会话目录）。
    // 只认受信连接（`trusted_only`）。
    ("directoryPicker", true),
    // `item/tool_completed.attachments` + `item/image`：工具结果里的截图等。
    ("toolImages", true),
    // `canvas/list` / `get` / `setData` / `rollback`：模型写的画布（HTML 分版 + 数据）。
    ("canvas", true),
    // `context/compacted`（压缩进展推送）+ `item/compaction`（压缩完成标记）+
    // `thread/environment/get` 的 `context.lastCompaction`。
    ("compactionProgress", true),
    // `subagent/list` / `history` / `send` / `interrupt` / `stop` + 推送 `subagent/updated`
    // （状态）与 `subagent/event`（子代理自己的对话事件，包在父线程上）。
    ("subagents", true),
    // `thread/context/get`：上下文窗口按类别拆开（系统提示 / 工具定义 / 消息 / 其余 / 空闲，
    // 各带明细）+ 按需加载、不占窗口的 MCP / 本地工具。
    ("contextBreakdown", true),
    // `schedule/list` / `create` / `update` / `delete` + 连接级推送 `schedule/changed`：
    // 定时任务（落盘、属于某个会话、跨重启继续跑）。
    ("schedules", true),
    // `vcs/pr/list` / `vcs/pr/get`：项目的 GitHub PR（只读，经本机 gh）。gh 用不了回
    // `available: false` + `reason` / `hint`，不是错误。
    ("pullRequests", true),
    // `thread/rewind`：撤回任意一条用户消息——它和之后的对话全部删掉，正文和图片还回来。
    ("threadRewind", true),
    // `turn/steer` 是插话（下一个步骤边界并进正在跑的一轮，不打断）而不是停止并发送；
    // 另有 `turn/queue/steer`，`turn/queue/list` 列出等着送达的插话（`kind: "steer"`），
    // `item/user_message` / 被它收尾的 `turn/completed` 带 `steered`。
    ("turnSteer", true),
    // `surface/list` / `get` / `action` + 连接级推送 `surface/changed { id }`：插件的面板
    // （`"tui.slots"`）。正文是文本，动作是按钮；`action` 只认受信连接。
    ("surfaces", true),
    // `surface/web { id }` → `{ html }`：面板自带的 web 界面（`surface/list` / `get` 带 `web`）。
    // GUI 放进沙箱 iframe，经窄桥读会话数据。只认受信连接。
    ("surfaceWeb", true),
    // `status/list` + 连接级推送 `status/changed { id }`：插件的状态项（短文字 + 色调，
    // 点它开插件面板）；`hud` 插槽也在里面。只认受信连接。
    ("statusItems", true),
    // `tool/views` / `tool/view` + 推送 `tool/views/changed`：插件给工具登记的卡片视图，
    // 展开工具卡时按需取（`tool/view` 只认受信连接）。
    ("toolViews", true),
    // `plugin/settings/list|get|set` + 推送 `plugin/settings/changed`：插件声明的设置卡，
    // 设置页按 schema 生成表单。只认受信连接。
    ("pluginSettings", true),
];

pub fn capabilities_object() -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in CAPABILITIES {
        map.insert((*k).into(), Value::Bool(*v));
    }
    Value::Object(map)
}

pub const CONNECTION_AUTHENTICATE: &str = "connection/authenticate";
pub const INITIALIZE: &str = "initialize";
pub const WORKSPACE_LIST: &str = "workspace/list";
pub const THREAD_LIST: &str = "thread/list";
pub const THREAD_SEARCH: &str = "thread/search";
pub const PRESET_LIST: &str = "preset/list";
pub const PRESET_CREATE: &str = "preset/create";
pub const PRESET_DELETE: &str = "preset/delete";
pub const PRESET_GET: &str = "preset/get";
pub const PRESET_UPDATE: &str = "preset/update";
pub const TOOL_CATALOG: &str = "tool/catalog";
pub const VCS_PR_LIST: &str = "vcs/pr/list";
pub const VCS_PR_GET: &str = "vcs/pr/get";
pub const SCHEDULE_LIST: &str = "schedule/list";
pub const SCHEDULE_CREATE: &str = "schedule/create";
pub const SCHEDULE_UPDATE: &str = "schedule/update";
pub const SCHEDULE_DELETE: &str = "schedule/delete";
/// 连接级推送（不用订阅线程）：定时任务变了，重拉 `schedule/list`。
pub const SCHEDULE_CHANGED: &str = "schedule/changed";
pub const SURFACE_LIST: &str = "surface/list";
pub const SURFACE_GET: &str = "surface/get";
pub const SURFACE_ACTION: &str = "surface/action";
pub const SURFACE_WEB: &str = "surface/web";
/// 连接级推送：插件面板增删了、被操作了或正文要重画（`{ id }`），重拉 `surface/get`。
pub const SURFACE_CHANGED: &str = "surface/changed";
pub const STATUS_LIST: &str = "status/list";
/// 连接级推送：状态项增删改了（`{ id }`），重拉 `status/list`。
pub const STATUS_CHANGED: &str = "status/changed";
pub const TOOL_VIEWS: &str = "tool/views";
pub const TOOL_VIEW: &str = "tool/view";
/// 连接级推送：有工具的卡片视图登记或卸下了（`{ name }`），重拉 `tool/views`。
pub const TOOL_VIEWS_CHANGED: &str = "tool/views/changed";
pub const PLUGIN_SETTINGS_LIST: &str = "plugin/settings/list";
pub const PLUGIN_SETTINGS_GET: &str = "plugin/settings/get";
pub const PLUGIN_SETTINGS_SET: &str = "plugin/settings/set";
/// 连接级推送：插件的设置卡登记 / 卸下了或值改了（`{ pluginId }`）。
pub const PLUGIN_SETTINGS_CHANGED: &str = "plugin/settings/changed";
pub const PRESET_DRAFT: &str = "preset/draft";
pub const PRESET_REWRITE: &str = "preset/rewrite";
pub const PRESET_SUGGEST_TOOLS: &str = "preset/suggestTools";
pub const THREAD_START: &str = "thread/start";
pub const THREAD_OPEN: &str = "thread/open";
pub const THREAD_CLOSE: &str = "thread/close";
pub const THREAD_RENAME: &str = "thread/rename";
pub const THREAD_ARCHIVE: &str = "thread/archive";
pub const THREAD_RESTORE: &str = "thread/restore";
pub const THREAD_DELETE: &str = "thread/delete";
pub const THREAD_HISTORY: &str = "thread/history";
pub const THREAD_REWIND: &str = "thread/rewind";
/// 一条工具结果里的一张图的像素（`item/tool_completed` 的 `attachments` 只带元数据）。
pub const ITEM_IMAGE: &str = "item/image";
pub const THREAD_SUBSCRIBE: &str = "thread/subscribe";
pub const THREAD_UNSUBSCRIBE: &str = "thread/unsubscribe";
pub const THREAD_ENVIRONMENT_GET: &str = "thread/environment/get";
pub const THREAD_MODEL_SET: &str = "thread/model/set";
pub const THREAD_MODEL_REFRESH: &str = "thread/model/refresh";
pub const THREAD_REASONING_SET: &str = "thread/reasoning/set";
pub const THREAD_APPROVAL_SET: &str = "thread/approval/set";
pub const THREAD_PLAN_SET: &str = "thread/plan/set";
pub const THREAD_MEMORY_SET: &str = "thread/memory/set";
pub const THREAD_GOAL_SET: &str = "thread/goal/set";
pub const THREAD_GOAL_EDIT: &str = "thread/goal/edit";
pub const THREAD_GOAL_PAUSE: &str = "thread/goal/pause";
pub const THREAD_GOAL_COMPLETE: &str = "thread/goal/complete";
pub const THREAD_GOAL_CLEAR: &str = "thread/goal/clear";
pub const THREAD_CONTEXT_COMPACT: &str = "thread/context/compact";
pub const THREAD_CONTEXT_GET: &str = "thread/context/get";
pub const TURN_START: &str = "turn/start";
pub const TURN_ENQUEUE: &str = "turn/enqueue";
pub const TURN_STEER: &str = "turn/steer";
pub const TURN_CANCEL: &str = "turn/cancel";
pub const TURN_QUEUE_LIST: &str = "turn/queue/list";
pub const TURN_QUEUE_REMOVE: &str = "turn/queue/remove";
pub const TURN_QUEUE_STEER: &str = "turn/queue/steer";
pub const PERMISSION_RESOLVE: &str = "permission/resolve";
pub const INTERACTION_RESPOND: &str = "interaction/respond";
pub const PLAN_RESOLVE: &str = "plan/resolve";
pub const ELICIT_RESOLVE: &str = "elicit/resolve";
pub const MCP_RELOAD: &str = "mcp/reload";
pub const MCP_LIST: &str = "mcp/list";
pub const MCP_RECONNECT: &str = "mcp/reconnect";
pub const MODEL_LIST: &str = "model/list";

// 设置页（只给受信连接，见 `handlers::settings::is_settings_method`）。
pub const CONFIG_STATUS: &str = "config/status";
pub const CONFIG_GET: &str = "config/get";
pub const CONFIG_SET: &str = "config/set";
pub const CONFIG_ENV: &str = "config/env";
pub const MODEL_GET: &str = "model/get";
pub const MODEL_SAVE: &str = "model/save";
pub const MODEL_DELETE: &str = "model/delete";
pub const MODEL_DEFAULT: &str = "model/default";
pub const MODEL_TEST: &str = "model/test";
pub const MCP_GET: &str = "mcp/get";
pub const MCP_SAVE: &str = "mcp/save";
pub const MCP_DELETE: &str = "mcp/delete";
pub const MCP_ENABLE: &str = "mcp/enable";
pub const MCP_TOOL_ENABLE: &str = "mcp/tool/enable";
pub const MCP_LOGIN: &str = "mcp/login";
pub const CUA_STATUS: &str = "cua/status";
pub const CUA_ACTION: &str = "cua/action";
pub const BROWSER_STATUS: &str = "browser/status";
pub const PLUGIN_LIST: &str = "plugin/list";
pub const PLUGIN_ENABLE: &str = "plugin/enable";
pub const PLUGIN_DELETE: &str = "plugin/delete";
pub const PLUGIN_PROMOTE: &str = "plugin/promote";
pub const PLUGIN_DISCARD: &str = "plugin/discard";
pub const SKILL_LIST: &str = "skill/list";
pub const SECRET_LIST: &str = "secret/list";
pub const SECRET_SET: &str = "secret/set";
pub const SECRET_DELETE: &str = "secret/delete";
pub const PAIRING_LIST: &str = "pairing/list";
pub const PAIRING_RESOLVE: &str = "pairing/resolve";
pub const PAIRING_REVOKE: &str = "pairing/revoke";
pub const PAIRING_ACCEPT: &str = "pairing/accept";
pub const DEVICE_LIST: &str = "device/list";
pub const DEVICE_ADD: &str = "device/add";
pub const DEVICE_REVOKE: &str = "device/revoke";
pub const SLASH_LIST: &str = "slash/list";
pub const SLASH_EXECUTE: &str = "slash/execute";
pub const IMAGE_INPUTS_SYNC: &str = "imageInputs/sync";
pub const IMAGE_INPUTS_PUT: &str = "imageInputs/put";

/// 一个线程启动的子代理（当前状态，和推送 `subagent/updated` 的 `agent` 同形）。
pub const SUBAGENT_LIST: &str = "subagent/list";
/// 一个子代理的对话事件（和推送 `subagent/event` 的 `event` 同形）。只在内存里：
/// Dock 重启后没有。
pub const SUBAGENT_HISTORY: &str = "subagent/history";
/// 用户对子代理说一句：在跑就在下一步读到，空闲就开下一轮。
pub const SUBAGENT_SEND: &str = "subagent/send";
/// 打断子代理这一轮（留着，能接着聊）。
pub const SUBAGENT_INTERRUPT: &str = "subagent/interrupt";
/// 收掉子代理（不能再接着聊）。
pub const SUBAGENT_STOP: &str = "subagent/stop";

pub const BROWSER_VIEW_OPEN: &str = "browser/view/open";
pub const BROWSER_VIEW_INPUT: &str = "browser/view/input";
pub const BROWSER_VIEW_NAVIGATE: &str = "browser/view/navigate";
pub const BROWSER_VIEW_CLOSE: &str = "browser/view/close";
/// 面板大小变了：按新的 CSS 尺寸和设备像素比改页面视口（agent 看到的也一起变）。
pub const BROWSER_VIEW_RESIZE: &str = "browser/view/resize";
/// 面板的标签栏：切到 / 新开 / 关掉会话的一个标签页（经浏览器 MCP，agent 也跟着切）。
pub const BROWSER_VIEW_TAB: &str = "browser/view/tab";
/// 推送：一帧画面（base64 JPEG + 视口元数据）。
pub const BROWSER_VIEW_FRAME: &str = "browser/view/frame";
/// 推送：视图换了标签页，或页面地址 / 标题变了。
pub const BROWSER_VIEW_STATUS: &str = "browser/view/status";
/// 推送：会话的标签页列表变了（`tabs`：`[{targetId, url, title, active}]`，按标签栏顺序）。
pub const BROWSER_VIEW_TABS: &str = "browser/view/tabs";
/// 推送：视图结束了（`reason`：`no_tab` 会话的标签页都关了 / `browser_exited`）。
pub const BROWSER_VIEW_CLOSED: &str = "browser/view/closed";

/// 桌面（CUA）面板的实时画面：agent 正在操作的窗口，经 cua-driver 只截图。
pub const DESKTOP_VIEW_OPEN: &str = "desktop/view/open";
pub const DESKTOP_VIEW_CLOSE: &str = "desktop/view/close";
/// 推送：一帧画面（base64 PNG + 窗口信息 + `source`：`agent` / `front`）。
pub const DESKTOP_VIEW_FRAME: &str = "desktop/view/frame";
/// 推送：视图结束了（`reason`：`driver_unavailable` / `replaced`）。
pub const DESKTOP_VIEW_CLOSED: &str = "desktop/view/closed";
/// 推送：agent 做了一次桌面动作——光标在窗口里的相对位置（0–1）+ 动作类型。
pub const DESKTOP_VIEW_CURSOR: &str = "desktop/view/cursor";
/// 推送：画面状态变了（`state`：`live` / `no_window` / `capture_failed`，带 `message`）。
pub const DESKTOP_VIEW_STATUS: &str = "desktop/view/status";

/// 只读看会话工作区：列一层目录 / 读一个文件 / 按名字找文件。
pub const FS_LIST: &str = "fs/list";
pub const FS_READ: &str = "fs/read";
pub const FS_FIND: &str = "fs/find";
/// 选目录：列这台机器上任意一个绝对路径下的子目录（远程 GUI 新建会话用）。只认受信连接。
pub const FS_DIRS: &str = "fs/dirs";

/// 会话的画布：列 / 读一版 / 改数据 / 回滚。
pub const CANVAS_LIST: &str = "canvas/list";
pub const CANVAS_GET: &str = "canvas/get";
pub const CANVAS_SET_DATA: &str = "canvas/setData";
pub const CANVAS_ROLLBACK: &str = "canvas/rollback";
pub const IMAGE_INPUTS_LIMITS_VERSION: u32 = 3;

#[derive(Clone, Debug)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub details_code: &'static str,
}

impl RpcError {
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            details_code: "method_not_found",
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            details_code: "invalid_params",
        }
    }

    pub fn app(details_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
            details_code,
        }
    }

    pub fn into_value(self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "details": { "code": self.details_code }
        })
    }
}

#[allow(dead_code)]
pub fn json_error(details_code: &'static str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": details_code, "message": message.into() } })
}

#[derive(Serialize)]
pub struct HttpErrorBody {
    pub error: HttpErrorInner,
}

#[derive(Serialize)]
pub struct HttpErrorInner {
    pub code: &'static str,
    pub message: String,
}

pub fn http_error(code: &'static str, message: impl Into<String>) -> HttpErrorBody {
    HttpErrorBody {
        error: HttpErrorInner {
            code,
            message: message.into(),
        },
    }
}

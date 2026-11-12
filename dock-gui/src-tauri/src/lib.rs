mod dock;
mod mcp_app;
mod pty;
mod remote;
mod tunnel;
#[cfg(target_os = "macos")]
mod webkit;

use dock::DockProcess;
use pty::Terminals;
use remote::{Remote, RemoteStore, Remotes, SshTarget};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{Emitter, Manager, RunEvent};
use tunnel::Tunnels;

/// 一次 WS 连接要的东西。本机：`ticket`（一次性，每次要新的）；远程：设备令牌 `token`。
/// `tunnel`：「测试连接」开的临时 SSH 隧道，测完前端拿它调 `remote_tunnel_close`。
#[derive(Serialize)]
struct Connection {
    ws: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel: Option<u16>,
}

fn tunnel_ws(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/api/ws")
}

/// 前端每次建 WS 连接前调一次。`id` 空 = 本机 Dock（一张新 ticket）；否则是那台远程
/// （设备令牌，走 SSH 的先确保这条连接自己的隧道，不拆别的）。
#[tauri::command]
async fn dock_connection(
    dock: tauri::State<'_, DockProcess>,
    remotes: tauri::State<'_, RemoteStore>,
    tunnels: tauri::State<'_, Tunnels>,
    id: Option<String>,
) -> Result<Connection, String> {
    let dock = dock.inner().clone();
    let remotes = remotes.inner().clone();
    let tunnels = tunnels.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            let remote = remotes.get(&id)?;
            let token = remotes.token(&remote.id)?;
            let ws = match &remote.ssh {
                Some(ssh) => tunnel_ws(tunnels.ensure(&remote.id, ssh)?),
                None => remote::ws_url(&remote.url)?,
            };
            return Ok(Connection {
                ws,
                ticket: None,
                token: Some(token),
                tunnel: None,
            });
        }
        let local = dock.connection()?;
        Ok(Connection {
            ws: local.ws,
            ticket: Some(local.ticket),
            token: None,
            tunnel: None,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 连接名单（不含令牌）、打开了哪些、主界面正在看哪台（`foreground` 空 = 本机）。
#[tauri::command]
fn remote_list(remotes: tauri::State<'_, RemoteStore>) -> Remotes {
    remotes.load()
}

/// 新建 / 改一个远程连接。令牌只进钥匙串；改的时候 `token` 空着就保留原来的。
/// 改的是已经打开的那条（换了令牌 / 地址 / SSH）：只拆它自己的隧道，让那条连接重连。
/// 本机 Dock 和别的远程连接不动，页面不重载。
#[tauri::command]
async fn remote_save(
    app: tauri::AppHandle,
    remotes: tauri::State<'_, RemoteStore>,
    tunnels: tauri::State<'_, Tunnels>,
    id: Option<String>,
    name: String,
    url: String,
    ssh: Option<SshTarget>,
    token: Option<String>,
) -> Result<Remote, String> {
    let remotes = remotes.inner().clone();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        let saved = remotes.save(id, name, url, ssh, token)?;
        let opened = remotes.load().opened.iter().any(|x| x == &saved.id);
        Ok::<_, String>((saved, opened))
    })
    .await
    .map_err(|e| e.to_string())??;
    if saved.1 {
        tunnels.stop_id(&saved.0.id);
        let _ = app.emit("dock://gateway-reconnect", &saved.0.id);
    }
    Ok(saved.0)
}

/// 删一个远程连接。它正打开着就拆掉它的隧道；本机 Dock 继续跑，页面不重载。
#[tauri::command]
async fn remote_delete(
    app: tauri::AppHandle,
    remotes: tauri::State<'_, RemoteStore>,
    tunnels: tauri::State<'_, Tunnels>,
    id: String,
) -> Result<(), String> {
    let store = remotes.inner().clone();
    let was_open = {
        let id = id.clone();
        tauri::async_runtime::spawn_blocking(move || store.delete(&id))
    }
    .await
    .map_err(|e| e.to_string())??;
    if was_open {
        tunnels.stop_id(&id);
        let _ = app.emit("dock://gateway-closed", &id);
    }
    Ok(())
}

/// 打开一条远程连接：记下来，主界面可以切过去。不关本机 Dock，也不拆别的隧道。
#[tauri::command]
fn gateway_open(remotes: tauri::State<'_, RemoteStore>, id: String) -> Result<(), String> {
    remotes.open_id(&id)
}

/// 关掉一条远程连接的 GUI 侧（套接字由前端关，这里拆隧道、从打开列表拿掉）。
/// 远端的 `dock serve` 继续跑。
#[tauri::command]
fn gateway_close(
    app: tauri::AppHandle,
    remotes: tauri::State<'_, RemoteStore>,
    tunnels: tauri::State<'_, Tunnels>,
    id: String,
) -> Result<(), String> {
    remotes.close_id(&id)?;
    tunnels.stop_id(&id);
    // 独立窗是另一份页面：靠这个事件停掉自己的重连。主窗口先把 store 卸了，再收到也是空操作。
    let _ = app.emit("dock://gateway-closed", &id);
    Ok(())
}

/// 主界面改看哪一台（`id` 空 = 本机）。不碰进程和隧道。
#[tauri::command]
fn gateway_focus(remotes: tauri::State<'_, RemoteStore>, id: Option<String>) -> Result<(), String> {
    remotes.focus(id.filter(|id| !id.is_empty()))
}

/// 「测试连接」：某个已存连接的地址和令牌（前端自己连一下试）。走 SSH 的开一条临时隧道。
#[tauri::command]
async fn remote_connection(
    remotes: tauri::State<'_, RemoteStore>,
    tunnels: tauri::State<'_, Tunnels>,
    id: String,
) -> Result<Connection, String> {
    let remotes = remotes.inner().clone();
    let tunnels = tunnels.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let remote = remotes.get(&id)?;
        let token = remotes.token(&id)?;
        let (ws, tunnel) = match &remote.ssh {
            Some(ssh) => {
                let port = tunnels.open_temp(ssh)?;
                (tunnel_ws(port), Some(port))
            }
            None => (remote::ws_url(&remote.url)?, None),
        };
        Ok(Connection {
            ws,
            ticket: None,
            token: Some(token),
            tunnel,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 「测试连接」还没存的 SSH 配置：开一条临时隧道，回地址和隧道 id。
#[tauri::command]
async fn remote_tunnel_open(
    tunnels: tauri::State<'_, Tunnels>,
    ssh: SshTarget,
) -> Result<Connection, String> {
    let tunnels = tunnels.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let port = tunnels.open_temp(&ssh.normalized()?)?;
        Ok(Connection {
            ws: tunnel_ws(port),
            ticket: None,
            token: None,
            tunnel: Some(port),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn remote_tunnel_close(tunnels: tauri::State<'_, Tunnels>, tunnel: u16) {
    tunnels.close_temp(tunnel);
}

/// 用户填的地址规范成 WS 地址（「测试连接」还没存的时候用），不合规矩回错。
#[tauri::command]
fn remote_ws_url(url: String) -> Result<String, String> {
    remote::ws_url(&url)
}

/// 独立窗 URL 带上它所属的网关（空 = 本机），这样这扇窗连同一台 Dock，而不是永远连本机。
fn with_gateway(url: String, gateway: &Option<String>) -> String {
    let Some(id) = gateway.as_deref().filter(|id| !id.is_empty()) else {
        return url;
    };
    let join = if url.contains('?') { '&' } else { '?' };
    format!("{url}{join}gateway={}", url_component(id))
}

/// 导出一段文本（画布的 HTML / 数据）：弹「存储为」对话框，用户选了位置才写。
/// 写到哪儿只由对话框决定，前端给不了路径——画布内容出自模型，不开任意写盘的口子。
/// 回写下的路径；用户取消回 `None`。
#[tauri::command]
async fn save_text_file(
    app: tauri::AppHandle,
    name: String,
    contents: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app.dialog().file().set_file_name(&name);
        if let Some(ext) = std::path::Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
        {
            dialog = dialog.add_filter(ext.to_uppercase(), &[ext]);
        }
        dialog.blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked else { return Ok(None) };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, contents).map_err(|e| format!("写文件失败：{e}"))?;
    Ok(Some(path.display().to_string()))
}

/// 工作台把一格拖出窗口：给这个会话开一扇独立的聊天窗。
///
/// 新窗口是另一个 WebView，自己调 `dock_connection` 连一份 WS（ticket 一次性，
/// 复用不了），但连的是同一个 Dock 进程：两边订阅同一批线程，事件各推一份，
/// 状态自然对齐。窗口 label 形如 `chat-<会话 id>`，已经开着就只是聚焦。
#[tauri::command]
async fn open_chat_window(
    app: tauri::AppHandle,
    session_id: String,
    gateway: Option<String>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let gw = gateway.as_deref().filter(|id| !id.is_empty());
    let label = match gw {
        Some(id) => format!("chat-{}-{}", safe_label(id), safe_label(&session_id)),
        None => format!("chat-{}", safe_label(&session_id)),
    };
    let url = with_gateway(format!("index.html?session={session_id}"), &gateway);
    open_detached(&app, &label, url, (760.0, 840.0), x, y)
}

/// 右侧面板（浏览器 / 文件 / 桌面 / 画布 / 子代理）拖出主窗口：开一扇只画这个面板的窗，跟着这个会话。
/// label 形如 `panel-<种类>-<会话 id>`：同一会话同一种面板只开一扇，已经开着就只是聚焦。
///
/// 插件面板（`plugin:<面板 id>`）不属于哪个会话：label 是 `panel-plugin-<面板 id>`，
/// 一个插件面板全局只开一扇；窗口小一点（监控 / 遥控用）。
///
/// MCP App 常驻面板（`mcp-app:<server>:<uri>`，两段各自 encodeURIComponent）跟着拖出时的会话
/// （项目也就定了）：label 是 `panel-mcpapp-<kind 的哈希>-<会话 id>`。kind 原样转进 URL 要再编码一层。
#[tauri::command]
async fn open_panel_window(
    app: tauri::AppHandle,
    panel: String,
    session_id: String,
    gateway: Option<String>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    if panel.starts_with("mcp-app:") {
        if !valid_app_panel(&panel) {
            return Err(format!("MCP App 面板不合法：{panel}"));
        }
        let label = match gateway.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => format!(
                "panel-mcpapp-{:08x}-{}-{}",
                fnv32(&panel),
                safe_label(id),
                safe_label(&session_id)
            ),
            None => format!(
                "panel-mcpapp-{:08x}-{}",
                fnv32(&panel),
                safe_label(&session_id)
            ),
        };
        let url = with_gateway(
            format!(
                "index.html?session={session_id}&panel={}",
                url_component(&panel)
            ),
            &gateway,
        );
        return open_detached(&app, &label, url, (1000.0, 760.0), x, y);
    }
    if let Some(id) = panel.strip_prefix("plugin:") {
        if !valid_plugin_panel(id) {
            return Err(format!("插件面板 id 不合法：{id}"));
        }
        let label = match gateway.as_deref().filter(|g| !g.is_empty()) {
            Some(gw) => format!("panel-plugin-{}-{}", safe_label(gw), safe_label(id)),
            None => format!("panel-plugin-{}", safe_label(id)),
        };
        let url = with_gateway(format!("index.html?panel=plugin:{id}"), &gateway);
        return open_detached(&app, &label, url, (440.0, 680.0), x, y);
    }
    if !matches!(
        panel.as_str(),
        "browser" | "files" | "desktop" | "canvas" | "subagents"
    ) {
        return Err(format!("未知面板：{panel}"));
    }
    let label = match gateway.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!(
            "panel-{panel}-{}-{}",
            safe_label(id),
            safe_label(&session_id)
        ),
        None => format!("panel-{panel}-{}", safe_label(&session_id)),
    };
    let url = with_gateway(
        format!("index.html?session={session_id}&panel={panel}"),
        &gateway,
    );
    open_detached(&app, &label, url, (900.0, 760.0), x, y)
}

/// 插件面板已经开成独立窗了就把它提到前面，回 `true`；没开回 `false`（调用方在主窗口右侧打开）。
/// 点状态项时用：别在主窗口再开一份。
#[tauri::command]
fn focus_panel_window(app: tauri::AppHandle, panel: String, gateway: Option<String>) -> bool {
    let Some(id) = panel.strip_prefix("plugin:") else {
        return false;
    };
    if !valid_plugin_panel(id) {
        return false;
    }
    let label = match gateway.as_deref().filter(|g| !g.is_empty()) {
        Some(gw) => format!("panel-plugin-{}-{}", safe_label(gw), safe_label(id)),
        None => format!("panel-plugin-{}", safe_label(id)),
    };
    match app.get_webview_window(&label) {
        Some(win) => {
            let _ = win.unminimize();
            win.set_focus().is_ok()
        }
        None => false,
    }
}

/// `mcp-app:<server>:<uri>`：两段都是 encodeURIComponent 的输出（只有这些字符），冒号正好两个，最长 1024。
fn valid_app_panel(kind: &str) -> bool {
    let Some(rest) = kind.strip_prefix("mcp-app:") else {
        return false;
    };
    kind.len() <= 1024
        && rest.split(':').count() == 2
        && rest.split(':').all(|part| !part.is_empty())
        && rest.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')' | '%' | ':'
                )
        })
}

/// 进 URL 查询串的一段：字母数字和 `-_.~` 以外都按字节转 `%XX`。
fn url_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// FNV-1a 32 位：窗口 label 里给 MCP App 面板一个短而稳定的名字。
fn fnv32(s: &str) -> u32 {
    s.bytes().fold(0x811c9dc5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    })
}

/// 插件面板 id 进窗口 label 和 URL：只认字母数字和 `_` `-` `.`，最长 64。
fn valid_plugin_panel(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// 窗口 label 只能是字母数字和 `-`。
fn safe_label(id: &str) -> String {
    id.replace(|c: char| !c.is_alphanumeric(), "-")
}

/// 开一扇独立窗（聊天 / 面板共用）；`x`、`y` 都是 0 = 系统默认位置。
fn open_detached(
    app: &tauri::AppHandle,
    label: &str,
    url: String,
    size: (f64, f64),
    x: f64,
    y: f64,
) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(label) {
        return win.set_focus().map_err(|e| e.to_string());
    }
    // 落在拖手松开的地方，不要每扇都叠在屏幕左上角；夹回屏幕内，免得开在看不见的位置。
    let (w, h) = size;
    let pos = (x > 0.0 || y > 0.0).then(|| {
        let (mut x, mut y) = (x, y);
        if let Ok(Some(mon)) = app.monitor_from_point(x, y) {
            let sf = mon.scale_factor();
            let pos = mon.position().to_logical::<f64>(sf);
            let size = mon.size().to_logical::<f64>(sf);
            x = x.clamp(pos.x, (pos.x + size.width - w).max(pos.x));
            y = y.clamp(pos.y, (pos.y + size.height - h).max(pos.y));
        }
        (x, y)
    });
    #[cfg(target_os = "macos")]
    {
        // webview 配置只能在主线程建（webkit.rs）：整个 build 挪到主线程，结果捎回来。
        let (tx, rx) = std::sync::mpsc::channel();
        let (handle, label) = (app.clone(), label.to_string());
        app.run_on_main_thread(move || {
            let mtm = objc2::MainThreadMarker::new().expect("run_on_main_thread 在主线程");
            let built = detached_builder(&handle, &label, url, size, pos)
                .with_webview_configuration(webkit::configuration(mtm))
                .build();
            let _ = tx.send(built.map(|_| ()).map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    detached_builder(app, label, url, size, pos)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn detached_builder<'a>(
    app: &'a tauri::AppHandle,
    label: &str,
    url: String,
    (w, h): (f64, f64),
    pos: Option<(f64, f64)>,
) -> tauri::WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle> {
    let mut builder =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(url.into()))
            .title("Dock")
            .inner_size(w, h)
            .min_inner_size(420.0, 400.0);
    #[cfg(target_os = "macos")]
    {
        // 透明窗口（同主窗口）：玻璃材质下网页放透明，露出 macOS 原生材质（prefs.ts 的 syncNativeGlass）；
        // 平面模式靠页面自己的不透底。
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(16.0, 18.0))
            .transparent(true);
    }
    if let Some((x, y)) = pos {
        builder = builder.position(x, y);
    }
    builder
}

/// 工作台格子 / 右侧面板的拖手按下时调：等鼠标左键松开（最多一分钟），松手时光标在发起拖拽的
/// 窗口外，就在光标处开独立窗：带 `panel` 开那个面板，不带开这个会话的聊天窗。
/// 回 `true` = 开了（或聚焦了已开的那扇）。
///
/// 不靠 WebView 的 pointerup：光标出了窗口以后 WKWebView 还收不收得到松手不可靠，
/// 按下还会被当成选字。这里直接问系统鼠标键状态。
#[tauri::command]
async fn pop_out_drag(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    session_id: String,
    panel: Option<String>,
    gateway: Option<String>,
) -> Result<bool, String> {
    let released = tauri::async_runtime::spawn_blocking(|| {
        let start = std::time::Instant::now();
        while left_button_down() {
            if start.elapsed() > std::time::Duration::from_secs(60) {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        true
    })
    .await
    .map_err(|e| e.to_string())?;
    if !released {
        return Ok(false);
    }
    let cursor = app.cursor_position().map_err(|e| e.to_string())?;
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let inside = cursor.x >= pos.x as f64
        && cursor.x < pos.x as f64 + size.width as f64
        && cursor.y >= pos.y as f64
        && cursor.y < pos.y as f64 + size.height as f64;
    if inside {
        return Ok(false);
    }
    let sf = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(window.scale_factor().map_err(|e| e.to_string())?);
    // 新窗口左上角稍微让开光标，松手处正好落在它的头栏上。
    let (x, y) = (cursor.x / sf - 40.0, cursor.y / sf - 16.0);
    match panel {
        Some(panel) => open_panel_window(app, panel, session_id, gateway, x, y).await?,
        None => open_chat_window(app, session_id, gateway, x, y).await?,
    }
    Ok(true)
}

/// 系统此刻鼠标左键按没按着。只在 macOS 上问得到；别的平台当作已经松开（拖出不生效）。
#[cfg(target_os = "macos")]
fn left_button_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
    }
    // kCGEventSourceStateCombinedSessionState = 0，kCGMouseButtonLeft = 0。
    unsafe { CGEventSourceButtonState(0, 0) }
}

#[cfg(not(target_os = "macos"))]
fn left_button_down() -> bool {
    false
}

/// 设置页「关于」：GUI 拉起的是哪个 Dock 可执行文件（规范化后的绝对路径）。
#[tauri::command]
fn dock_path() -> String {
    let bin = dock::dock_bin();
    std::fs::canonicalize(&bin)
        .unwrap_or(bin)
        .display()
        .to_string()
}

/// Dock 退出后，「重新启动」按钮调这个。
#[tauri::command]
fn dock_restart(app: tauri::AppHandle, dock: tauri::State<'_, DockProcess>) {
    dock.restart(app);
}

/// 底部终端：开一个登录 shell，输出走 `out`。返回终端 id。
#[tauri::command]
fn pty_spawn(
    webview: tauri::Webview,
    terms: tauri::State<'_, Terminals>,
    cwd: String,
    cols: u16,
    rows: u16,
    out: Channel,
) -> Result<u32, String> {
    terms.spawn(webview.label(), &cwd, cols, rows, out)
}

#[tauri::command]
fn pty_write(terms: tauri::State<'_, Terminals>, id: u32, data: String) -> Result<(), String> {
    terms.write(id, &data)
}

#[tauri::command]
fn pty_resize(
    terms: tauri::State<'_, Terminals>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    terms.resize(id, cols, rows)
}

#[tauri::command]
fn pty_kill(terms: tauri::State<'_, Terminals>, id: u32) {
    terms.kill(id);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let dock = DockProcess::default();
    let terms = Terminals::default();
    let tunnels = Tunnels::default();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(dock.clone())
        .manage(terms.clone())
        // 页面重载（开发时热更新、⌘R）：旧页面的终端标签没了，它开的 shell 一并收掉，不留孤儿。
        .on_page_load({
            let terms = terms.clone();
            move |webview, payload| {
                if payload.event() == tauri::webview::PageLoadEvent::Started {
                    terms.kill_owned_by(webview.label());
                }
            }
        })
        .manage(RemoteStore::default())
        // MCP App 界面的独立源（mcp_app.rs）：iframe 拿真实源，才能给 allow-same-origin。
        .manage(mcp_app::AppDocs::default())
        .register_uri_scheme_protocol(mcp_app::SCHEME, mcp_app::protocol)
        .manage(tunnels.clone())
        .setup(|app| {
            // 主窗口照 tauri.conf.json 的那份配置建（"create": false），macOS 上带 webkit.rs 的配置：
            // 60 帧封顶的开关只能在建 webview 前关。
            let main = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned()
                .expect("tauri.conf.json 缺 main 窗口");
            let builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &main)?;
            #[cfg(target_os = "macos")]
            let builder = builder.with_webview_configuration(webkit::configuration(
                objc2::MainThreadMarker::new().expect("setup 在主线程"),
            ));
            builder.build()?;
            let remotes = app.state::<RemoteStore>();
            remotes.init(app.handle());
            // 本机 Dock 跟 GUI 同寿命。看远程的时候它也留着，上面的回合不会被停掉。
            app.state::<DockProcess>().spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dock_connection,
            remote_list,
            remote_save,
            remote_delete,
            remote_connection,
            remote_tunnel_open,
            remote_tunnel_close,
            remote_ws_url,
            gateway_open,
            gateway_close,
            gateway_focus,
            dock_restart,
            dock_path,
            open_chat_window,
            open_panel_window,
            focus_panel_window,
            pop_out_drag,
            save_text_file,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            mcp_app::mcp_app_doc
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |app, event| match event {
            // 窗口没了告诉各个窗口（载荷是 label）：关窗时那扇窗自己发的事件不一定发得出去，
            // 别的窗口靠这条把它开着的面板（MCP App 跨窗口路由）认作已关。
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } => {
                let _ = app.emit("window/destroyed", label);
            }
            RunEvent::Exit => {
                dock.shutdown();
                terms.kill_all();
                tunnels.stop_all();
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_panel_kinds_are_checked_and_encoded() {
        // 前端 panelKind("whiteboard", "ui://whiteboard/board.html")。
        let kind = "mcp-app:whiteboard:ui%3A%2F%2Fwhiteboard%2Fboard.html";
        assert!(valid_app_panel(kind));
        assert!(!valid_app_panel("mcp-app:whiteboard"), "少一段");
        assert!(!valid_app_panel("mcp-app:a:b:c"), "多一段");
        assert!(!valid_app_panel("mcp-app::b"), "空段");
        assert!(!valid_app_panel("mcp-app:a:ui://x"), "没编码的 / 不认");
        assert!(!valid_app_panel("mcp-app:a&session=x:b"), "& 会拆查询串");
        assert!(!valid_app_panel(&format!("mcp-app:a:{}", "b".repeat(1100))));
        // 再编码一层：前端 query.get("panel") 解一层，拿回原样的 kind。
        assert_eq!(
            url_component(kind),
            "mcp-app%3Awhiteboard%3Aui%253A%252F%252Fwhiteboard%252Fboard.html"
        );
        assert_ne!(fnv32(kind), fnv32("mcp-app:whiteboard:ui%3A%2F%2Fother"));
    }
}

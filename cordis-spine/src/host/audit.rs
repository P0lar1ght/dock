//! 沙箱的「观测」和「审计」：开着沙箱（`/sandbox on`，见 [`crate::host::sandbox`]）的页，
//! 它做过什么都记进一份和聊天记录分开的审计日志（`$DOCK_HOME/audit/<开始时刻>-<会话>/`，
//! 格式见 [`dock_sandbox::audit::store`]）：
//!
//! - **工具调用**：`tools/pre-execute` 记调用（工具、参数），`tools/execute` 记结果（成败、长度、
//!   开头一段、全文哈希）。
//! - **命令**：`jobs` 起进程时（[`for_command`]）记命令、退出码、耗时，并给它开一个出口代理口、
//!   注入代理和证书的环境变量——命令和它的子进程走代理的出网（HTTPS 在代理处解密）都记下来，
//!   出网目的地还要过沙箱策略（[`Audit::rules_for`]）。
//! - **内置浏览器**：MCP 调用的 `_meta["dock/audit"]` 带上审计目录，浏览器服务把这个会话
//!   标签页的网络请求写进同一个目录（`browser.jsonl`）。浏览器里看到的本来就是明文。
//!
//! **沙箱关着时什么都不做**：不起代理、不注入环境变量、不写盘，开销为零。
//! 子代理（含工作流派的）记进派它那一页的审计（`Sessions::ui_page`）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::config::{dock_home, load_sandbox_config, RuleAction, SandboxRule};
use cordis_base::types::{PreExecute, ToolResult};
use dock_sandbox::audit::ca::Authority;
use dock_sandbox::audit::proxy::{Gate, Learn, Listener, Options, Proxy, Sink, Upstream};
use dock_sandbox::audit::record::Flow;
use dock_sandbox::audit::store::AuditLog;
use dock_sandbox::rules::{self, Action, Rule};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::host::permissions::{Decision, Permissions};
use crate::names::{AUDIT, PERMISSIONS, SESSIONS, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE};
use crate::session::log::Sessions;

/// 工具参数最多记这么多字符（`write_file` 的全文可能很长；全文哈希另记）。
const ARGS_MAX: usize = 16 * 1024;
/// 工具结果记开头这么多字符。
const RESULT_HEAD: usize = 4 * 1024;
/// 命令结束后，它的出口代理口还认这么久（连接在命令退出后才关、记录晚到）。
const ROUTE_GRACE: Duration = Duration::from_secs(30);
const EVENTS: &str = "events.jsonl";
const PASSTHROUGH: &str = "passthrough.json";

/// 一页正在进行的审计。
pub struct Active {
    pub dir: PathBuf,
    pub(crate) log: Arc<AuditLog>,
    /// 记的是哪个会话（目录名的后半截）。页换了会话就另开一份。
    session: String,
}

/// 出口代理和 Dock 根证书（第一次有命令要用时才起）。
struct Egress {
    proxy: Arc<Proxy>,
    certs: Option<(PathBuf, PathBuf)>,
    no_proxy: String,
    decrypt: bool,
}

/// 出口代理口（命令的 job id）属于谁：记进哪份日志、出网要确认时在哪一页问。
#[derive(Clone)]
struct Route {
    log: Arc<AuditLog>,
    page: Context,
}

type Routes = Arc<Mutex<HashMap<String, Route>>>;

/// named `"audit"`。
pub struct Audit {
    /// 页（`ui_page`）→ 这一页正在进行的审计（沙箱开着、第一次有东西要记时开）。
    pages: Mutex<HashMap<String, Arc<Active>>>,
    /// 页 → 这次任务里临时放行的规则（`/sandbox allow …`），排在配置的规则前面。
    grants: Mutex<HashMap<String, Vec<Rule>>>,
    egress: OnceLock<Result<Arc<Egress>, String>>,
    routes: Routes,
}

impl Audit {
    fn new() -> Self {
        Self {
            pages: Mutex::new(HashMap::new()),
            grants: Mutex::new(HashMap::new()),
            egress: OnceLock::new(),
            routes: Arc::default(),
        }
    }

    /// `ctx`（一页或子代理）归哪一页。
    fn page_of(ctx: &Context) -> Option<(String, Arc<Sessions>)> {
        let sessions = ctx.get::<Sessions>(SESSIONS)?;
        let page = sessions
            .ui_page()
            .unwrap_or_else(|| sessions.identity().to_string());
        Some((page, sessions))
    }

    /// `ctx` 所在那一页正在进行的审计：沙箱没开就是 `None`；开着就第一次碰到时开一份。
    ///
    /// 这一页换了会话（`/resume`、恢复了别的会话）时，前一个会话的审计记一条「结束」，
    /// 新会话另开一份：目录按会话命名，会话的历史（[`session_runs`]）才不会混进别人的记录。
    /// 只在页自己的上下文里比对：子代理的会话 id 不是这一页的，沿用这一页正在记的那份。
    pub fn active(&self, ctx: &Context) -> Option<Arc<Active>> {
        if !crate::host::sandbox::enabled(ctx) {
            return None;
        }
        let (page, sessions) = Self::page_of(ctx)?;
        let session = session_of(&page, &sessions);
        let mut pages = self.pages.lock().unwrap();
        if let Some(active) = pages.get(&page) {
            if !sessions.is_main() || active.session == session {
                return Some(active.clone());
            }
            active.log.record(
                "audit",
                json!({ "event": "结束", "reason": "这一页换了会话" }),
            );
        }
        let active = Arc::new(start(&page, &session, &sessions).ok()?);
        pages.insert(page, active.clone());
        Some(active)
    }

    /// 这一页此刻在记的那份（不新开）：沙箱关着、还没记过、换了会话都是 `None`。
    fn current(&self, page: &Context) -> Option<Arc<Active>> {
        let (key, sessions) = Self::page_of(page)?;
        let session = session_of(&key, &sessions);
        self.pages
            .lock()
            .unwrap()
            .get(&key)
            .filter(|a| a.session == session)
            .cloned()
    }

    /// 沙箱关了：结束这一页的审计（记一条「结束」），这次任务的临时放行也作废。返回目录。
    pub fn close(&self, ctx: &Context) -> Option<PathBuf> {
        let (page, _) = Self::page_of(ctx)?;
        self.grants.lock().unwrap().remove(&page);
        let previous = self.pages.lock().unwrap().remove(&page);
        previous.map(|active| {
            active.log.record("audit", json!({ "event": "结束" }));
            active.dir.clone()
        })
    }

    /// 这次任务里临时放行一条（`/sandbox allow`）：排在配置的规则前面，沙箱关掉就作废。
    pub fn grant(&self, ctx: &Context, rule: Rule) {
        if let Some((page, _)) = Self::page_of(ctx) {
            if let Some(active) = self.active(ctx) {
                active.log.record(
                    "audit",
                    json!({ "event": "临时放行", "host": rule.host, "command": rule.command }),
                );
            }
            self.grants
                .lock()
                .unwrap()
                .entry(page)
                .or_default()
                .push(rule);
        }
    }

    /// 这一页生效的规则：临时放行的在前，配置的（`[[sandbox.rules]]`）在后。
    pub fn rules_for(&self, ctx: &Context) -> Vec<Rule> {
        let mut out = Self::page_of(ctx)
            .and_then(|(page, _)| self.grants.lock().unwrap().get(&page).cloned())
            .unwrap_or_default();
        out.extend(load_sandbox_config().rules.iter().map(to_rule));
        out
    }

    fn egress(&self) -> Result<Arc<Egress>, String> {
        self.egress
            .get_or_init(|| start_egress(self.routes.clone()).map(Arc::new))
            .clone()
    }

    /// 直通名单（客户端不信 Dock 根证书、原样转发的主机）：`(主机, 落了盘)`。落盘的是确定被拒的，
    /// [`PASSTHROUGH_TTL`] 内有效；没落盘的只在这个进程里（说不准是不信证书还是连接被取消）。
    pub(crate) fn passthrough(&self) -> Vec<(String, bool)> {
        let saved = load_passthrough(&dock_home().join("audit"));
        let mut hosts: Vec<(String, bool)> = saved.iter().map(|h| (h.clone(), true)).collect();
        if let Some(Ok(egress)) = self.egress.get() {
            for host in egress.proxy.passthrough_hosts() {
                if !saved.contains(&host) {
                    hosts.push((host, false));
                }
            }
        }
        hosts.sort();
        hosts
    }

    /// 从直通名单里拿掉 `host`（`None` = 全部），盘上和正在跑的出口代理一起：之后又解密。
    /// 回拿掉了几个。
    pub(crate) fn forget_passthrough(&self, host: Option<&str>) -> usize {
        let before = self.passthrough();
        forget_saved_passthrough(&dock_home().join("audit"), host);
        if let Some(Ok(egress)) = self.egress.get() {
            egress.proxy.forget_passthrough(host);
        }
        before.len() - self.passthrough().len()
    }
}

fn to_rule(r: &SandboxRule) -> Rule {
    Rule {
        host: r.host.clone(),
        command: r.command.clone(),
        action: match r.action {
            RuleAction::Allow => Action::Allow,
            RuleAction::Ask => Action::Ask,
            RuleAction::Deny => Action::Deny,
        },
    }
}

/// 出网目的地过一遍策略：`None` 放行，`Some(原因)` 阻断。要问的在 `page` 上问（权限档位、
/// 「始终允许」照常算——这一页点过「始终允许」就是这次任务授权放行了）。
pub(crate) async fn check_host(page: &Context, host: &str, port: Option<u16>) -> Option<String> {
    let audit = page.get::<Audit>(AUDIT)?;
    let rule = rules::for_host(&audit.rules_for(page), host)?.clone();
    let pattern = rule.host.clone().unwrap_or_default();
    match rule.action {
        Action::Allow => None,
        Action::Deny => Some(format!("规则 {pattern} → deny")),
        Action::Ask => {
            let Some(perms) = page.get::<Permissions>(PERMISSIONS) else {
                return Some(format!("规则 {pattern} → ask，但这里没人能确认"));
            };
            let target = match port {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            let summary = format!("沙箱策略：要连 {target}（规则 {pattern} → ask）");
            match perms.decide(&format!("net:{host}"), &summary, None).await {
                Decision::Allowed => None,
                Decision::Denied => Some("用户没有放行".into()),
                Decision::TimedOut(_) => Some("等确认超时".into()),
            }
        }
    }
}

/// 一页记在哪个会话名下：落盘的会话 id，还没有就是页名。
fn session_of(page: &str, sessions: &Sessions) -> String {
    match sessions.live_session_id() {
        id if id.is_empty() => page.to_string(),
        id => id,
    }
}

/// 审计目录名的后半截（`<时间>-<会话>`）。
fn dir_suffix(session: &str) -> String {
    session.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_")
}

/// `YYYYMMDD-HHMMSS`（[`stamp`]）的长度。
const STAMP_LEN: usize = 15;

/// 一个会话记过的每一次（Dock 每起一次、沙箱开着就各开一份目录），按时间先后。
/// Dock 重启后打开老会话，行为轨迹靠它把之前几次的记录接上。
pub(crate) fn session_runs(session: &str) -> Vec<PathBuf> {
    let suffix = format!("-{}", dir_suffix(session));
    let is_stamp = |s: &str| {
        s.len() == STAMP_LEN
            && s.char_indices()
                .all(|(i, c)| if i == 8 { c == '-' } else { c.is_ascii_digit() })
    };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dock_home().join("audit"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(&suffix).is_some_and(is_stamp)
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// 这一页的会话记过的每一次（见 [`session_runs`]）。
fn page_runs(page: &Context) -> Vec<PathBuf> {
    match Audit::page_of(page) {
        Some((key, sessions)) => session_runs(&session_of(&key, &sessions)),
        None => Vec::new(),
    }
}

/// 新开一份审计目录。
fn start(page: &str, session: &str, sessions: &Sessions) -> Result<Active, String> {
    let dir = dock_home()
        .join("audit")
        .join(format!("{}-{}", stamp(), dir_suffix(session)));
    let log =
        AuditLog::open(&dir, EVENTS).map_err(|e| format!("开审计目录 {}：{e}", dir.display()))?;
    let cfg = load_sandbox_config();
    log.record(
        "audit",
        json!({
            "event": "开始",
            "page": page,
            "session": session,
            "cwd": sessions.workspace_cwd().map(|p| p.display().to_string()),
            "workspace": sessions.space(),
            "decrypt": cfg.decrypt,
            "dock": env!("CARGO_PKG_VERSION"),
        }),
    );
    Ok(Active {
        dir,
        log: Arc::new(log),
        session: session.to_string(),
    })
}

fn stamp() -> String {
    let now = time_now();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.0, now.1, now.2, now.3, now.4, now.5
    )
}

/// 本地时间的年月日时分秒（不为这一处拉时区库：按 UTC 偏移走 `date`）。
fn time_now() -> (i64, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local = chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.with_timezone(&chrono::Local).naive_local())
        .unwrap_or_default();
    use chrono::{Datelike, Timelike};
    (
        local.year() as i64,
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    )
}

fn start_egress(routes: Routes) -> Result<Egress, String> {
    let cfg = load_sandbox_config();
    let root = dock_home().join("audit");
    let ca = if cfg.decrypt {
        Some(Arc::new(Authority::load_or_create(&root.join("ca"))?))
    } else {
        None
    };
    let certs = match &ca {
        Some(ca) => Some((ca.cert_path(), ca.bundle()?)),
        None => None,
    };
    let sink_routes = routes.clone();
    let sink: Sink = Arc::new(move |flow: Flow| {
        let log = sink_routes
            .lock()
            .unwrap()
            .get(&flow.tag)
            .map(|r| r.log.clone());
        if let Some(log) = log {
            log.flow(flow);
        }
    });
    // 出网先过沙箱策略：按发起它的那条命令找到页，在那一页上判、要问就在那一页上问。
    let gate: Gate = Arc::new(move |tag: &str, host: &str, port: u16| {
        let page = routes.lock().unwrap().get(tag).map(|r| r.page.clone());
        let host = host.to_string();
        Box::pin(async move { check_host(&page?, &host, Some(port)).await })
    });
    let learned_root = root.clone();
    let learn: Learn =
        Arc::new(move |host: &str, why: &str| save_passthrough(&learned_root, host, why));
    let (proxy_url, no_proxy) = user_proxy();
    let upstream = proxy_url.and_then(|url| Upstream::parse(&url, &no_proxy));
    let proxy = Proxy::with_gate(
        ca,
        Options {
            decrypt: cfg.decrypt,
            body_cap: (cfg.body_cap_kb as usize) * 1024,
            redact: cfg.redact_secrets,
            upstream,
            extra_roots: Vec::new(),
        },
        sink,
        load_passthrough(&root),
        Some(learn),
        Some(gate),
    );
    Ok(Egress {
        proxy,
        certs,
        no_proxy,
        decrypt: cfg.decrypt,
    })
}

const PROXY_KEYS: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// 用户自己的代理（命令本来走的那个），出口代理往外连时经它走——不然本来要靠它才连得上
/// 外网的命令，经过审计反而连不上了。先看 Dock 进程的环境；没有就问一次登录 shell
/// （命令是 `bash -lc` 起的，`~/.bash_profile` 里 export 的代理 Dock 自己的环境里不一定有：
/// 从桌面端启动时就没有）。
fn user_proxy() -> (Option<String>, String) {
    let from_env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let url = PROXY_KEYS.iter().find_map(|k| from_env(k));
    let no_proxy = from_env("NO_PROXY").or_else(|| from_env("no_proxy"));
    if url.is_some() {
        return (url, no_proxy.unwrap_or_default());
    }
    let script = PROXY_KEYS
        .iter()
        .chain(["NO_PROXY", "no_proxy"].iter())
        .map(|k| format!("printf '%s=%s\\n' {k} \"${k}\""))
        .collect::<Vec<_>>()
        .join("; ");
    // 只在出口代理第一次起时问一次，但 profile 可能很慢（nvm 之类）：最多等
    // [`LOGIN_SHELL_WAIT`]，到点就当没有上游代理，不卡住起命令。
    let Some(stdout) = login_shell_output(&script) else {
        return (None, no_proxy.unwrap_or_default());
    };
    let text = String::from_utf8_lossy(&stdout);
    let value = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let url = PROXY_KEYS.iter().find_map(|k| value(k));
    let no_proxy = no_proxy
        .or_else(|| value("NO_PROXY"))
        .or_else(|| value("no_proxy"))
        .unwrap_or_default();
    (url, no_proxy)
}

const LOGIN_SHELL_WAIT: Duration = Duration::from_secs(3);

/// `bash -lc <script>` 的标准输出；超过 [`LOGIN_SHELL_WAIT`] 没跑完就杀掉、返回 `None`。
fn login_shell_output(script: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut child = std::process::Command::new("bash")
        .arg("-lc")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + LOGIN_SHELL_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = Vec::new();
    child.stdout.take()?.read_to_end(&mut out).ok()?;
    Some(out)
}

/// 单引号转义（`'` → `'\''`）。
fn shell_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "'\\''"))
}

/// 落了盘的直通记录多久有效：过期的不再认，下次又试着解密（客户端可能已经信了 Dock 根证书）。
const PASSTHROUGH_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// 之前学到的直通主机（客户端确定拒绝过 Dock 根证书），过期的不算。
fn load_passthrough(root: &Path) -> Vec<String> {
    let now = dock_sandbox::audit::record::now_ms();
    let mut hosts: Vec<String> = read_passthrough(root)
        .into_iter()
        .filter(|(_, v)| {
            let ts = v.get("ts").and_then(Value::as_u64).unwrap_or(0);
            now.saturating_sub(ts) < PASSTHROUGH_TTL_MS
        })
        .map(|(host, _)| host)
        .collect();
    hosts.sort();
    hosts
}

fn read_passthrough(root: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(root.join(PASSTHROUGH))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(&s).ok())
        .unwrap_or_default()
}

fn forget_saved_passthrough(root: &Path, host: Option<&str>) {
    let path = root.join(PASSTHROUGH);
    if !path.exists() {
        return;
    }
    let mut map = read_passthrough(root);
    match host {
        Some(h) => {
            map.remove(&h.to_ascii_lowercase());
        }
        None => map.clear(),
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&map).unwrap_or_default(),
    );
}

fn save_passthrough(root: &Path, host: &str, why: &str) {
    let path = root.join(PASSTHROUGH);
    let mut map = read_passthrough(root);
    map.insert(
        host.to_ascii_lowercase(),
        json!({ "reason": why, "ts": dock_sandbox::audit::record::now_ms() }),
    );
    let _ = std::fs::create_dir_all(root);
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&map).unwrap_or_default(),
    );
}

/// 一条被审计的命令：起进程前加上 [`CommandAudit::env`]，跑完调 [`CommandAudit::finish`]。
pub(crate) struct CommandAudit {
    pub env: Vec<(String, String)>,
    job: String,
    log: Arc<AuditLog>,
    routes: Routes,
    started: Instant,
    _listener: Option<Listener>,
}

impl CommandAudit {
    /// 真正交给 `bash -lc` 的命令：前面补上这些 `export`。命令是登录 shell 起的，
    /// `~/.bash_profile` 里自己 export 的代理会盖掉进程环境里的——写在命令前面才算数。
    pub fn wrap(&self, command: &str) -> String {
        if self.env.is_empty() {
            return command.to_string();
        }
        let exports: Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", shell_quote(v)))
            .collect();
        format!("export {};\n{command}", exports.join(" "))
    }

    pub fn finish(self, exit: Option<i32>) {
        self.log.record(
            "command_end",
            json!({
                "job": self.job,
                "exit": exit,
                "ms": self.started.elapsed().as_millis() as u64,
            }),
        );
        let routes = self.routes.clone();
        let job = self.job.clone();
        tokio::spawn(async move {
            tokio::time::sleep(ROUTE_GRACE).await;
            routes.lock().unwrap().remove(&job);
        });
    }
}

/// `jobs` 起命令时调：调用页开着审计就记下这条命令、给它开出口代理口。没开就是 `None`
/// （什么都不做）。要在 tokio 运行时里调。
pub(crate) fn for_command(
    job: &str,
    command: &str,
    is_monitor: bool,
    cwd: Option<&Path>,
) -> Option<CommandAudit> {
    let exec = crate::tools::registry::exec_ctx()?;
    let audit = exec.get::<Audit>(AUDIT)?;
    let active = audit.active(&exec)?;
    active.log.record(
        "command",
        json!({
            "job": job,
            "call_id": crate::tools::registry::current_call_id(),
            "tool": if is_monitor { "monitor" } else { "bash" },
            "program": program_of(command),
            "command": command,
            "cwd": cwd.map(|p| p.display().to_string()),
        }),
    );
    let mut env = Vec::new();
    let mut listener = None;
    match audit.egress() {
        Ok(egress) => match egress.proxy.listen_now(job) {
            Ok(l) => {
                audit.routes.lock().unwrap().insert(
                    job.to_string(),
                    Route {
                        log: active.log.clone(),
                        page: exec.clone(),
                    },
                );
                let certs = egress
                    .certs
                    .as_ref()
                    .map(|(ca, bundle)| (ca.as_path(), bundle.as_path()));
                env = dock_sandbox::audit::env::command_env(l.port, certs, &egress.no_proxy);
                listener = Some(l);
            }
            Err(e) => active.log.record(
                "audit",
                json!({ "event": "出口代理开不了口", "error": e.to_string() }),
            ),
        },
        Err(e) => active
            .log
            .record("audit", json!({ "event": "出口代理不可用", "error": e })),
    }
    Some(CommandAudit {
        env,
        job: job.to_string(),
        log: active.log.clone(),
        routes: audit.routes.clone(),
        started: Instant::now(),
        _listener: listener,
    })
}

/// 命令里真正干活的程序（链路图上的一个节点）：跳过 `cd`、`export`、`FOO=1` 这类，取路径的最后一段。
pub(crate) fn program_of(command: &str) -> String {
    const SKIP: &[&str] = &[
        "cd", "export", "set", "source", ".", "env", "sudo", "time", "exec", "nohup", "command",
    ];
    for segment in command.split(['&', ';', '|', '\n']) {
        for word in segment.split_whitespace() {
            let word = word.trim_matches(|c| c == '(' || c == ')' || c == '"' || c == '\'');
            if word.is_empty() || word.contains('=') || word.starts_with('-') {
                continue;
            }
            if SKIP.contains(&word) {
                break;
            }
            return word.rsplit('/').next().unwrap_or(word).to_string();
        }
    }
    "sh".into()
}

/// 浏览器服务写审计用的目录：调用页开着审计时，给内置浏览器的 `tools/call` 带上。
pub(crate) fn calling_audit_dir() -> Option<PathBuf> {
    let exec = crate::tools::registry::exec_ctx()?;
    let audit = exec.get::<Audit>(AUDIT)?;
    audit.active(&exec).map(|a| a.dir.clone())
}

/// 内置浏览器要套的主机规则（按顺序；`ask` 子请求问不了人，不带）：`(通配, 阻断?)`。
pub(crate) fn calling_host_rules() -> Vec<(String, bool)> {
    let Some(exec) = crate::tools::registry::exec_ctx() else {
        return Vec::new();
    };
    let Some(audit) = exec.get::<Audit>(AUDIT) else {
        return Vec::new();
    };
    if audit.active(&exec).is_none() {
        return Vec::new();
    }
    audit
        .rules_for(&exec)
        .into_iter()
        .filter_map(|r| match r.action {
            Action::Allow => r.host.map(|h| (h, false)),
            Action::Deny => r.host.map(|h| (h, true)),
            Action::Ask => None,
        })
        .collect()
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn on_pre_execute(pre: &PreExecute) {
    let Some(exec) = crate::tools::registry::exec_ctx() else {
        return;
    };
    let Some(audit) = exec.get::<Audit>(AUDIT) else {
        return;
    };
    let Some(active) = audit.active(&exec) else {
        return;
    };
    let args = pre.arguments();
    active.log.record(
        "tool_call",
        json!({
            "tool": pre.name(),
            "called": pre.called,
            "call_id": crate::tools::registry::current_call_id(),
            "identity": pre.identity,
            "args": clip(args, ARGS_MAX),
            "args_len": args.len(),
            "args_sha256": sha256_hex(args),
            "denied": pre.denial(),
        }),
    );
}

fn on_execute(result: &ToolResult) {
    let Some(exec) = crate::tools::registry::exec_ctx() else {
        return;
    };
    let Some(audit) = exec.get::<Audit>(AUDIT) else {
        return;
    };
    let Some(active) = audit.active(&exec) else {
        return;
    };
    active.log.record(
        "tool_result",
        json!({
            "tool": result.name,
            "call_id": result.call_id,
            "is_error": result.is_error,
            "result_head": clip(&result.content, RESULT_HEAD),
            "result_len": result.content.len(),
            "result_sha256": sha256_hex(&result.content),
            "images": result.images.len(),
        }),
    );
}

/// dock.1 `sandbox/status`：这一页沙箱的现状，给界面画。会验一遍哈希链和正文块（读整份日志），
/// 不要每秒调；随时间变的看 [`sandbox_trace`]。
pub fn sandbox_status(page: &Context) -> Value {
    let cfg = load_sandbox_config();
    let audit = page.get::<Audit>(AUDIT);
    let enabled = crate::host::sandbox::enabled(page);
    // 这个会话记过的每一次都算：Dock 重启过，之前的记录照样在、照样验。
    let runs = page_runs(page);
    let records = dock_sandbox::audit::store::read_runs(&runs).len();
    let intact = runs.iter().all(|dir| {
        dock_sandbox::audit::store::verify(dir)
            .iter()
            .all(|v| v.intact())
    });
    let dir = audit
        .as_ref()
        .and_then(|a| a.current(page))
        .map(|a| a.dir.clone())
        .or_else(|| runs.last().cloned());
    let rules: Vec<Value> = audit
        .as_ref()
        .map(|a| a.rules_for(page))
        .unwrap_or_default()
        .iter()
        .map(|r| {
            json!({
                "host": r.host,
                "command": r.command,
                "action": match r.action {
                    Action::Allow => "allow",
                    Action::Ask => "ask",
                    Action::Deny => "deny",
                },
            })
        })
        .collect();
    let passthrough: Vec<Value> = audit
        .as_ref()
        .map(|a| a.passthrough())
        .unwrap_or_default()
        .into_iter()
        .map(|(host, saved)| json!({ "host": host, "saved": saved }))
        .collect();
    let egress = match audit.as_ref().and_then(|a| a.egress.get()) {
        Some(Ok(_)) => "running",
        Some(Err(_)) => "unavailable",
        None => "idle",
    };
    // 系统沙箱后端（限制要靠它）：能用时还有这台机器上做不到的保证，不能用时有原因。
    let (backend, backend_reason, limits) = match dock_sandbox::availability() {
        dock_sandbox::Availability::Ready { backend, limits } => (
            Some(backend.name()),
            None,
            limits.iter().map(|l| l.describe()).collect::<Vec<_>>(),
        ),
        dock_sandbox::Availability::Unavailable { reason } => {
            (None, Some(reason.as_str()), Vec::new())
        }
    };
    json!({
        "enabled": enabled,
        "workspace": crate::session::cwd::session_space(page),
        "cwd": crate::session::cwd::session_cwd(page),
        "auditing": enabled,
        "dir": dir.map(|d| d.display().to_string()),
        "runs": runs.len(),
        "records": records,
        "intact": intact,
        "decrypt": cfg.decrypt,
        "egress": egress,
        "confine": cfg.confine,
        "network": match cfg.network {
            cordis_base::config::SandboxNetwork::Allow => "allow",
            cordis_base::config::SandboxNetwork::Proxy => "proxy",
            cordis_base::config::SandboxNetwork::Off => "off",
        },
        "backend": backend,
        "backendReason": backend_reason,
        "limits": limits,
        "rules": rules,
        "passthrough": passthrough,
    })
}

/// dock.1 `sandbox/trace`：这一页的会话的行为链路图（会话 → 工具 → 程序 → 主机，带次数、流量、
/// 出错）、概况和最近 `limit` 条时间线。会话记过的每一次都接上（Dock 重启之前的也在）；
/// 沙箱现在关着也照样给之前记下的。`auditing` = 现在开着、接着记。
pub fn sandbox_trace(page: &Context, limit: usize) -> Value {
    let runs = page_runs(page);
    let events = dock_sandbox::audit::store::read_runs(&runs);
    let auditing = crate::host::sandbox::enabled(page);
    if events.is_empty() {
        return json!({
            "auditing": auditing,
            "runs": runs.len(),
            "records": 0,
            "graph": { "nodes": [], "edges": [] },
            "totals": Value::Null,
            "timeline": [],
        });
    }
    let summary = dock_sandbox::audit::graph::Summary::build(&events);
    json!({
        "auditing": auditing,
        "runs": runs.len(),
        "records": events.len(),
        "graph": summary.graph(),
        "totals": {
            "toolCalls": summary.tool_calls,
            "commands": summary.commands,
            "requests": summary.requests,
            "decrypted": summary.decrypted,
            "bytes": summary.bytes,
            "errors": summary.errors,
        },
        "timeline": dock_sandbox::audit::graph::timeline(&events, limit),
    })
}

/// dock.1 `sandbox/http`：这一页的会话最近 `limit` 次出网（HTTP 历史，跨 Dock 重启），
/// 每条带 `id`（按它取完整的包）。
pub fn sandbox_http(page: &Context, limit: usize) -> Value {
    let runs = page_runs(page);
    let events = dock_sandbox::audit::store::read_runs(&runs);
    json!({
        "auditing": crate::host::sandbox::enabled(page),
        "runs": runs.len(),
        "records": events.len(),
        "requests": dock_sandbox::audit::graph::requests(&events, limit),
    })
}

/// dock.1 `sandbox/flow`：一次出网的请求包 / 响应包（首行、头、正文；压缩的解开）。
/// `id` 是 `<目录名>/<文件>:<序号>`（[`dock_sandbox::audit::store::read_runs`]），目录只认
/// 这一页的会话自己的那几份。老的 `<文件>:<序号>` 按最近一次找。没有这条是 `None`。
pub fn sandbox_flow(page: &Context, id: &str) -> Option<Value> {
    let runs = page_runs(page);
    let (dir, rest) = match id.split_once('/') {
        Some((name, rest)) => (
            runs.iter()
                .find(|d| d.file_name().is_some_and(|n| n == name))?,
            rest,
        ),
        None => (runs.last()?, id),
    };
    let mut detail = dock_sandbox::audit::packet::flow_detail(dir, rest)?;
    detail["id"] = json!(id);
    Some(detail)
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// `/sandbox graph`：这一页的会话记过的（跨 Dock 重启）概况、终端里的链路树，后面附 Mermaid
/// （桌面端画成图）。什么都没记过是 `None`。
pub(crate) fn graph_text(page: &Context) -> Option<String> {
    let events = dock_sandbox::audit::store::read_runs(&page_runs(page));
    if events.is_empty() {
        return None;
    }
    let summary = dock_sandbox::audit::graph::Summary::build(&events);
    Some(format!(
        "工具调用 {} · 命令 {} · 出网 {}（解密 {}）· 流量 {} · 失败 {}\n\n{}\n可视化报告：/sandbox export\n\n```mermaid\n{}```",
        summary.tool_calls,
        summary.commands,
        summary.requests,
        summary.decrypted,
        dock_sandbox::audit::graph::human_bytes(summary.bytes),
        summary.errors,
        summary.tree(),
        summary.mermaid()
    ))
}

/// `/sandbox export`：报告与原始记录，返回 `report.html` 的路径。
pub(crate) fn export(active: &Active) -> Result<PathBuf, String> {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    dock_sandbox::audit::graph::export(&active.dir, &format!("Dock 沙箱审计报告 #{n}"))
        .map(|out| out.join("report.html"))
}

/// `/sandbox status` 里审计那几行。
pub(crate) fn status_lines(audit: &Audit, page: &Context) -> Vec<String> {
    let mut out = Vec::new();
    let runs = page_runs(page);
    let records = dock_sandbox::audit::store::read_runs(&runs).len();
    let intact = runs.iter().all(|dir| {
        dock_sandbox::audit::store::verify(dir)
            .iter()
            .all(|v| v.intact())
    });
    let history = match runs.len() {
        0 => String::new(),
        1 => format!(
            "（{records} 条，哈希链{}）",
            if intact { "完好" } else { "对不上" }
        ),
        n => format!(
            "（这个会话 {n} 次运行共 {records} 条，哈希链{}）",
            if intact { "完好" } else { "对不上" }
        ),
    };
    match (audit.current(page), crate::host::sandbox::enabled(page)) {
        (Some(active), _) => out.push(format!("审计：{}{history}", active.dir.display())),
        (None, true) => out.push(format!("审计：开着，有东西要记时开一份新目录{history}")),
        (None, false) if !runs.is_empty() => {
            out.push(format!("审计：没在记（沙箱关着）；之前记过{history}"))
        }
        (None, false) => out.push("审计：没在记（沙箱开着才记）".into()),
    }
    match audit.egress.get() {
        Some(Ok(e)) => out.push(format!(
            "出口代理：已启动，HTTPS {}",
            if e.decrypt {
                "在代理处解密（Dock 根证书只经命令的环境变量信任，不装进系统）"
            } else {
                "不解密（只记目的地和字节数）"
            }
        )),
        Some(Err(e)) => out.push(format!("出口代理：不可用（{e}）")),
        None => out.push("出口代理：还没有命令用过".into()),
    }
    let passthrough = audit.passthrough();
    if !passthrough.is_empty() {
        let list = passthrough
            .iter()
            .map(|(h, saved)| {
                if *saved {
                    h.clone()
                } else {
                    format!("{h}（本次）")
                }
            })
            .collect::<Vec<_>>()
            .join("、");
        out.push(format!(
            "原样转发（客户端不信 Dock 根证书）的主机：{list}；/sandbox passthrough clear 清掉"
        ));
    }
    out
}

/// 挂 [`AUDIT`]，记工具调用（沙箱开着的页）。开关与命令在 `sandbox` 插件（`/sandbox`）。
pub fn audit() -> Plugin {
    plugin("audit", Inject::new(), |ctx, _: &()| {
        let service = ctx.provide(AUDIT, Audit::new())?;
        let pre = ctx
            .clone()
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let next = args.next::<PreExecute>().unwrap_or(pre);
                on_pre_execute(&next);
                next
            })?;
        let post = ctx
            .clone()
            .on_waterfall(TOOLS_EXECUTE, |result: ToolResult, args| {
                let next = args.next::<ToolResult>().unwrap_or(result);
                on_execute(&next);
                next
            })?;
        let all = ctx.effect("audit", |scope| {
            scope.own(service);
            scope.own(pre);
            scope.own(post);
            Ok(())
        })?;
        Ok(Some(all))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘的直通记录 30 天后不再认；`clear` 能清掉一个或全部。
    #[test]
    fn saved_passthrough_expires_and_can_be_cleared() {
        let dir = tempfile::tempdir().unwrap();
        save_passthrough(dir.path(), "Fresh.Example", "UnknownCA");
        save_passthrough(dir.path(), "other.example", "UnknownCA");
        let mut map = read_passthrough(dir.path());
        map.insert(
            "old.example".into(),
            json!({ "reason": "UnknownCA", "ts": 1 }),
        );
        std::fs::write(
            dir.path().join(PASSTHROUGH),
            serde_json::to_string(&map).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_passthrough(dir.path()),
            ["fresh.example", "other.example"],
            "过期的不认"
        );
        forget_saved_passthrough(dir.path(), Some("FRESH.example"));
        assert_eq!(load_passthrough(dir.path()), ["other.example"]);
        forget_saved_passthrough(dir.path(), None);
        assert!(load_passthrough(dir.path()).is_empty());
    }
    use crate::tools::registry::Tools;
    use cordis_base::types::ToolCall;

    async fn http_once() -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
                        .await;
                });
            }
        });
        port
    }

    /// 开了审计的页起的命令：命令本身、它经出口代理的出网（同一个 job）、退出码都记下来，
    /// 链是完好的。没开审计时 `for_command` 什么都不做。
    #[tokio::test(flavor = "multi_thread")]
    async fn an_audited_command_records_itself_and_its_egress() {
        if std::process::Command::new("curl")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let _env = cordis_base::test_env::scoped().home();
        let ctx = Context::new();
        for p in [
            crate::session::log::sessions(),
            crate::host::settings::settings(),
        ] {
            ctx.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        let _svc = ctx.provide(AUDIT, Audit::new()).unwrap();
        let audit = ctx.require::<Audit>(AUDIT).unwrap();
        let settings = ctx
            .require::<crate::host::settings::AppSettings>(crate::names::SETTINGS)
            .unwrap();
        let off = crate::tools::registry::with_exec_ctx_async(ctx.clone(), async {
            for_command("job-0", "true", false, None).is_none()
        })
        .await;
        assert!(off, "沙箱没开：不记、不起代理");

        settings.set_sandbox(true);
        let active = audit.active(&ctx).unwrap();
        let port = http_once().await;
        // 发往本机的默认不走代理（那不是出网）；这里强制走一回。
        let command = format!("curl -s --noproxy '' http://127.0.0.1:{port}/hi");
        let cmd = crate::tools::registry::with_exec_ctx_async(ctx.clone(), async {
            for_command("job-1", &command, false, None)
        })
        .await
        .expect("开着审计");
        let out = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(cmd.wrap(&command))
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
        cmd.finish(out.status.code());

        let mut events = Vec::new();
        for _ in 0..100 {
            events = dock_sandbox::audit::store::read(&active.dir);
            if events.iter().any(|e| e["kind"] == "command_end")
                && events.iter().any(|e| e["kind"] == "net")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let kind = |k: &str| {
            events
                .iter()
                .find(|e| e["kind"] == k)
                .cloned()
                .unwrap_or_default()
        };
        assert_eq!(kind("audit")["event"], "开始");
        assert_eq!(kind("command")["job"], "job-1");
        assert_eq!(kind("command")["program"], "curl");
        let net = kind("net");
        assert_eq!(net["tag"], "job-1", "出网追得到是哪条命令：{net}");
        assert_eq!(net["path"], "/hi");
        assert_eq!(net["status"], 200);
        assert_eq!(kind("command_end")["exit"], 0);
        assert!(dock_sandbox::audit::store::verify(&active.dir)
            .iter()
            .all(|v| v.intact()));
        settings.set_sandbox(false);
        assert!(audit.close(&ctx).is_some());
        assert!(audit.active(&ctx).is_none());
    }

    /// 工具调用经注册表跑：`tools/pre-execute` 记调用（参数、调用 id），`tools/execute` 记结果
    /// （成败、长度、哈希），两条的 `call_id` 对得上。
    #[tokio::test]
    async fn tool_calls_are_recorded_with_their_call_id() {
        let _env = cordis_base::test_env::scoped().home();
        let ctx = Context::new();
        for p in [
            crate::session::log::sessions(),
            crate::host::settings::settings(),
            audit(),
        ] {
            ctx.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        let tools = Tools::echo(ctx.clone());
        let _tool = tools
            .register(
                cordis_base::types::ToolSpec {
                    name: "ping".into(),
                    description: "ping".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                Arc::new(|call: ToolCall| {
                    Box::pin(async move { crate::tools::registry::tool_result(call, "pong") })
                }),
            )
            .unwrap();
        let audit = ctx.require::<Audit>(AUDIT).unwrap();
        ctx.require::<crate::host::settings::AppSettings>(crate::names::SETTINGS)
            .unwrap()
            .set_sandbox(true);
        let active = audit.active(&ctx).unwrap();
        let out = crate::tools::registry::with_exec_ctx_async(
            ctx.clone(),
            tools.execute_on(
                &ctx,
                ToolCall {
                    id: "call-7".into(),
                    name: "ping".into(),
                    arguments: r#"{"x":1}"#.into(),
                },
            ),
        )
        .await;
        assert_eq!(out.content, "pong");
        let mut events = Vec::new();
        for _ in 0..50 {
            events = dock_sandbox::audit::store::read(&active.dir);
            if events.iter().any(|e| e["kind"] == "tool_result") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let call = events.iter().find(|e| e["kind"] == "tool_call").unwrap();
        let result = events.iter().find(|e| e["kind"] == "tool_result").unwrap();
        assert_eq!(call["tool"], "ping");
        assert_eq!(call["args"], r#"{"x":1}"#);
        assert_eq!(call["call_id"], "call-7");
        assert_eq!(result["call_id"], "call-7");
        assert_eq!(result["result_head"], "pong");
        assert_eq!(result["is_error"], false);

        // 关了就不再记。
        ctx.require::<crate::host::settings::AppSettings>(crate::names::SETTINGS)
            .unwrap()
            .set_sandbox(false);
        audit.close(&ctx);
        // 「结束」那条是后台写的：等它落盘再数。
        tokio::time::sleep(Duration::from_millis(50)).await;
        let before = dock_sandbox::audit::store::read(&active.dir).len();
        assert_eq!(
            dock_sandbox::audit::store::read(&active.dir)
                .last()
                .unwrap()["event"],
            "结束"
        );
        crate::tools::registry::with_exec_ctx_async(
            ctx.clone(),
            tools.execute_on(
                &ctx,
                ToolCall {
                    id: "call-8".into(),
                    name: "ping".into(),
                    arguments: "{}".into(),
                },
            ),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(dock_sandbox::audit::store::read(&active.dir).len(), before);
    }

    #[test]
    fn exports_go_before_the_command_and_survive_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let log = rt.block_on(async { Arc::new(AuditLog::open(dir.path(), EVENTS).unwrap()) });
        let a = CommandAudit {
            env: vec![
                ("SSL_CERT_FILE".into(), "/a b/it's.pem".into()),
                ("HTTPS_PROXY".into(), "http://127.0.0.1:9".into()),
            ],
            job: "job-1".into(),
            log,
            routes: Arc::default(),
            started: Instant::now(),
            _listener: None,
        };
        let wrapped = a.wrap("echo \"$SSL_CERT_FILE\"");
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(&wrapped)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/a b/it's.pem");
        assert!(wrapped.ends_with("echo \"$SSL_CERT_FILE\""));
    }

    #[test]
    fn program_skips_setup_and_paths() {
        assert_eq!(program_of("cargo build -p x"), "cargo");
        assert_eq!(program_of("cd foo && RUST_LOG=1 /usr/bin/git pull"), "git");
        assert_eq!(program_of("FOO=1 npm i"), "npm");
        assert_eq!(program_of("(curl -s https://a)"), "curl");
        assert_eq!(program_of("   "), "sh");
    }
}

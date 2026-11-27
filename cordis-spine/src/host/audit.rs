//! `audit` 插件：开了审计的页，它做过什么都记进一份和聊天记录分开的审计日志
//! （`$DOCK_HOME/audit/<开始时刻>-<会话>/`，格式见 [`dock_audit::store`]）：
//!
//! - **工具调用**：`tools/pre-execute` 记调用（工具、参数），`tools/execute` 记结果（成败、长度、
//!   开头一段、全文哈希）。
//! - **命令**：`jobs` 起进程时（[`for_command`]）记命令、退出码、耗时，并给它开一个出口代理口、
//!   注入代理和证书的环境变量——命令和它的子进程走代理的出网（HTTPS 在代理处解密）都记下来。
//! - **内置浏览器**：MCP 调用的 `_meta["dock/audit"]` 带上审计目录，浏览器服务把这个会话
//!   标签页的网络请求写进同一个目录（`browser.jsonl`）。浏览器里看到的本来就是明文。
//!
//! 开关按页：新开的页按 `[audit] enabled`（默认关，插件挂载时读一次），`/audit on|off` 切
//! 当前页。**关着时什么都不做**：不起代理、不注入环境变量、不写盘，开销为零。
//! 子代理（含工作流派的）记进派它那一页的审计（`Sessions::ui_page`）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::config::{dock_home, load_audit_config};
use cordis_base::types::{PreExecute, ToolResult};
use dock_audit::ca::Authority;
use dock_audit::proxy::{Learn, Listener, Options, Proxy, Sink, Upstream};
use dock_audit::record::Flow;
use dock_audit::store::AuditLog;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{AUDIT, SESSIONS, SLASH, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE};
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
    log: Arc<AuditLog>,
}

/// 出口代理和 Dock 根证书（第一次有命令要用时才起）。
struct Egress {
    proxy: Arc<Proxy>,
    certs: Option<(PathBuf, PathBuf)>,
    no_proxy: String,
    decrypt: bool,
}

/// named `"audit"`。
pub struct Audit {
    default_on: bool,
    /// 页（`ui_page`）→ 开着的审计；`None` = 明确关了（`/audit off`），不再按默认开。
    pages: Mutex<HashMap<String, Option<Arc<Active>>>>,
    egress: OnceLock<Result<Arc<Egress>, String>>,
    /// 出口代理口的 tag（命令的 job id）→ 记进哪份日志。
    routes: Arc<Mutex<HashMap<String, Arc<AuditLog>>>>,
}

impl Audit {
    fn new(default_on: bool) -> Self {
        Self {
            default_on,
            pages: Mutex::new(HashMap::new()),
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

    /// `ctx` 所在那一页开着的审计（按默认值第一次碰到时开）。
    pub fn active(&self, ctx: &Context) -> Option<Arc<Active>> {
        let (page, sessions) = Self::page_of(ctx)?;
        let mut pages = self.pages.lock().unwrap();
        match pages.get(&page) {
            Some(state) => state.clone(),
            None if self.default_on => {
                let started = start(&page, &sessions).ok().map(Arc::new);
                pages.insert(page, started.clone());
                started
            }
            None => None,
        }
    }

    /// 开 `ctx` 这一页的审计；已经开着就返回原来那份。
    pub fn turn_on(&self, ctx: &Context) -> Result<Arc<Active>, String> {
        let (page, sessions) = Self::page_of(ctx).ok_or("这一页没有会话")?;
        let mut pages = self.pages.lock().unwrap();
        if let Some(Some(active)) = pages.get(&page) {
            return Ok(active.clone());
        }
        let active = Arc::new(start(&page, &sessions)?);
        pages.insert(page, Some(active.clone()));
        Ok(active)
    }

    /// 关 `ctx` 这一页的审计，返回刚才那份的目录。
    pub fn turn_off(&self, ctx: &Context) -> Option<PathBuf> {
        let (page, _) = Self::page_of(ctx)?;
        let previous = self.pages.lock().unwrap().insert(page, None).flatten();
        previous.map(|active| {
            active.log.record("audit", json!({ "event": "结束" }));
            active.dir.clone()
        })
    }

    fn egress(&self) -> Result<Arc<Egress>, String> {
        self.egress
            .get_or_init(|| start_egress(self.routes.clone()).map(Arc::new))
            .clone()
    }
}

/// 新开一份审计目录。
fn start(page: &str, sessions: &Sessions) -> Result<Active, String> {
    let session = match sessions.live_session_id() {
        id if id.is_empty() => page.to_string(),
        id => id,
    };
    let dir = dock_home().join("audit").join(format!(
        "{}-{}",
        stamp(),
        session.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_")
    ));
    let log =
        AuditLog::open(&dir, EVENTS).map_err(|e| format!("开审计目录 {}：{e}", dir.display()))?;
    let cfg = load_audit_config();
    log.record(
        "audit",
        json!({
            "event": "开始",
            "page": page,
            "session": session,
            "cwd": sessions.workspace_cwd().map(|p| p.display().to_string()),
            "decrypt": cfg.decrypt,
            "dock": env!("CARGO_PKG_VERSION"),
        }),
    );
    Ok(Active {
        dir,
        log: Arc::new(log),
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

fn start_egress(routes: Arc<Mutex<HashMap<String, Arc<AuditLog>>>>) -> Result<Egress, String> {
    let cfg = load_audit_config();
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
    let sink: Sink = Arc::new(move |flow: Flow| {
        let log = routes.lock().unwrap().get(&flow.tag).cloned();
        if let Some(log) = log {
            log.flow(flow);
        }
    });
    let learned_root = root.clone();
    let learn: Learn =
        Arc::new(move |host: &str, why: &str| save_passthrough(&learned_root, host, why));
    let (proxy_url, no_proxy) = user_proxy();
    let upstream = proxy_url.and_then(|url| Upstream::parse(&url, &no_proxy));
    let proxy = Proxy::new(
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

/// 之前学到的直通主机（客户端拒绝过 Dock 根证书）。
fn load_passthrough(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(PASSTHROUGH))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(&s).ok())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn save_passthrough(root: &Path, host: &str, why: &str) {
    let path = root.join(PASSTHROUGH);
    let mut map = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(&s).ok())
        .unwrap_or_default();
    map.insert(
        host.to_ascii_lowercase(),
        json!({ "reason": why, "ts": dock_audit::record::now_ms() }),
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
    routes: Arc<Mutex<HashMap<String, Arc<AuditLog>>>>,
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
                audit
                    .routes
                    .lock()
                    .unwrap()
                    .insert(job.to_string(), active.log.clone());
                let certs = egress
                    .certs
                    .as_ref()
                    .map(|(ca, bundle)| (ca.as_path(), bundle.as_path()));
                env = dock_audit::env::command_env(l.port, certs, &egress.no_proxy);
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

static SEQ: AtomicU64 = AtomicU64::new(0);

fn run_command(page: &Context, args: &str) -> SlashOutcome {
    let Some(audit) = page.get::<Audit>(AUDIT) else {
        return SlashOutcome::notice("审计", "审计服务没有挂载。");
    };
    let sub = args.split_whitespace().next().unwrap_or("status");
    match sub {
        "on" => match audit.turn_on(page) {
            Ok(active) => {
                SlashOutcome::Applied(format!("这一页开始审计：{}", active.dir.display()))
            }
            Err(e) => SlashOutcome::Applied(format!("开不了审计：{e}")),
        },
        "off" => match audit.turn_off(page) {
            Some(dir) => {
                SlashOutcome::Applied(format!("这一页停止审计（记录在 {}）", dir.display()))
            }
            None => SlashOutcome::Applied("这一页没有在审计".into()),
        },
        "graph" => match audit.active(page) {
            Some(active) => {
                let events = dock_audit::store::read(&active.dir);
                let summary = dock_audit::graph::Summary::build(&events);
                SlashOutcome::notice(
                    "审计链路图",
                    format!(
                        "工具调用 {} · 命令 {} · 出网 {}（解密 {}）· 流量 {} · 失败 {}\n\n{}\n可视化报告：/audit export\n\n```mermaid\n{}```",
                        summary.tool_calls,
                        summary.commands,
                        summary.requests,
                        summary.decrypted,
                        dock_audit::graph::human_bytes(summary.bytes),
                        summary.errors,
                        summary.tree(),
                        summary.mermaid()
                    ),
                )
            }
            None => SlashOutcome::Applied("这一页没有在审计（/audit on 开始）".into()),
        },
        "export" => match audit.active(page) {
            Some(active) => {
                let n = SEQ.fetch_add(1, Ordering::Relaxed);
                let title = format!("Dock 审计报告 #{n}");
                match dock_audit::graph::export(&active.dir, &title) {
                    Ok(out) => SlashOutcome::Applied(format!(
                        "已导出：{}",
                        out.join("report.html").display()
                    )),
                    Err(e) => SlashOutcome::Applied(format!("导出失败：{e}")),
                }
            }
            None => SlashOutcome::Applied("这一页没有在审计（/audit on 开始）".into()),
        },
        _ => SlashOutcome::notice("审计", status(&audit, page)),
    }
}

fn status(audit: &Audit, page: &Context) -> String {
    let mut out = String::new();
    match audit.active(page) {
        Some(active) => {
            let events = dock_audit::store::read(&active.dir);
            let verified = dock_audit::store::verify(&active.dir);
            let intact = verified.iter().all(|v| v.broken_at.is_none());
            out.push_str(&format!(
                "这一页：**审计中**\n记录：{}（{} 条，哈希链{}）\n",
                active.dir.display(),
                events.len(),
                if intact { "完好" } else { "对不上" }
            ));
        }
        None => out.push_str(
            "这一页：没有在审计（`/audit on` 开始；`[audit] enabled = true` 让新开的页默认开）\n",
        ),
    }
    match audit.egress.get() {
        Some(Ok(e)) => out.push_str(&format!(
            "出口代理：已启动，HTTPS {}\n",
            if e.decrypt {
                "在代理处解密（Dock 根证书只经命令的环境变量信任，不装进系统）"
            } else {
                "不解密（只记目的地和字节数）"
            }
        )),
        Some(Err(e)) => out.push_str(&format!("出口代理：不可用（{e}）\n")),
        None => out.push_str("出口代理：还没有命令用过\n"),
    }
    let passthrough = load_passthrough(&dock_home().join("audit"));
    if !passthrough.is_empty() {
        out.push_str(&format!(
            "原样转发（客户端不信 Dock 根证书）的主机：{}\n",
            passthrough.join("、")
        ));
    }
    out.push_str("\n`/audit on` · `/audit off` · `/audit graph` · `/audit export`");
    out
}

/// 挂 [`AUDIT`]，记工具调用；`/audit` 命令。
pub fn audit() -> Plugin {
    plugin("audit", Inject::from([SLASH]), |ctx, _: &()| {
        let service = ctx.provide(AUDIT, Audit::new(load_audit_config().enabled))?;
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
        register_commands(
            ctx,
            vec![SlashCommand::host(
                "audit",
                "审计：记录这一页的工具调用、命令和出网",
                slash_handler(run_command),
            )
            .takes_args(true)
            .hint("audit on", "开始审计这一页", false)
            .hint("audit off", "停止审计这一页", false)
            .hint("audit graph", "看这一页的行为链路图", false)
            .hint("audit export", "导出报告（HTML + 原始记录）", false)],
        )?;
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
        ctx.plugin(crate::session::log::sessions(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let _svc = ctx.provide(AUDIT, Audit::new(false)).unwrap();
        let audit = ctx.require::<Audit>(AUDIT).unwrap();
        let off = crate::tools::registry::with_exec_ctx_async(ctx.clone(), async {
            for_command("job-0", "true", false, None).is_none()
        })
        .await;
        assert!(off, "没开审计：不记、不起代理");

        let active = audit.turn_on(&ctx).unwrap();
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
            events = dock_audit::store::read(&active.dir);
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
        assert!(dock_audit::store::verify(&active.dir)
            .iter()
            .all(|v| v.broken_at.is_none()));
        assert!(audit.turn_off(&ctx).is_some());
    }

    /// 工具调用经注册表跑：`tools/pre-execute` 记调用（参数、调用 id），`tools/execute` 记结果
    /// （成败、长度、哈希），两条的 `call_id` 对得上。
    #[tokio::test]
    async fn tool_calls_are_recorded_with_their_call_id() {
        let _env = cordis_base::test_env::scoped().home();
        let ctx = Context::new();
        for p in [
            crate::session::log::sessions(),
            crate::host::slash::slash(),
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
        let active = audit.turn_on(&ctx).unwrap();
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
            events = dock_audit::store::read(&active.dir);
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
        audit.turn_off(&ctx);
        // 「结束」那条是后台写的：等它落盘再数。
        tokio::time::sleep(Duration::from_millis(50)).await;
        let before = dock_audit::store::read(&active.dir).len();
        assert_eq!(
            dock_audit::store::read(&active.dir).last().unwrap()["event"],
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
        assert_eq!(dock_audit::store::read(&active.dir).len(), before);
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

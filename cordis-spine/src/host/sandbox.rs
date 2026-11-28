//! `sandbox` 插件：沙箱 = 隔离 + 观测 + 控制 + 审计，**不减少 agent 能做的事**。按页开
//! （`/sandbox on|off`，新开的页按 `[sandbox] enabled` 播种）。开着的页：
//!
//! - **观测 / 审计**：工具调用、命令、命令和浏览器的出网都记下来（[`crate::host::audit`]）。
//! - **控制**：策略单独放（`[[sandbox.rules]]`），按出网目的地或命令放行 / 询问 / 阻断；
//!   默认放行。命令和 `web_fetch` / 浏览器跳转的网址在 `tools/pre-execute` 上判（[`on_policy`]），
//!   命令自己的出网在出口代理上判（[`crate::host::audit::check_host`]）。这次任务临时放行
//!   用 `/sandbox allow`。权限档位（询问 / 智能授权 / 始终允许）照旧，沙箱不替它放行。
//! - **限制**（默认不限）：`[sandbox] confine = true` 时 `bash` / `monitor` 在系统沙箱里跑
//!   （[`dock_sandbox`]：只能写工作区等、读不到密钥），这时沙箱里的命令不再弹权限框；
//!   `network = "proxy"` 让命令只能经审计代理出网（macOS 由内核挡住直连）。
//!
//! 后端不可用（内核没有 Landlock 之类）时限制不起作用：命令照常过权限门，不会不加防护地跑。
//!
//! 不变式：关卡判「沙箱里跑、不用问」（[`Gate::Sandboxed`]）的调用，执行时**一定**在沙箱里跑。
//! 这个决定由注册表经任务局部变量交给工具体（[`with_access`] / [`granted_access`]），不让
//! 工具体自己再判一次——判两次中间要是有人切了开关，就成了「没问、也没进沙箱」。包不上就
//! 报错不跑，绝不退回沙箱外。

use std::future::Future;
use std::path::{Path, PathBuf};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::config::SandboxNetwork;
use cordis_base::types::PreExecute;
use dock_sandbox::rules::{self, Action, Rule};
use dock_sandbox::{Access, Availability, Host, Policy, Settings};
use serde_json::Value;

use crate::host::audit::Audit;
use crate::host::settings::AppSettings;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{AUDIT, SETTINGS, SLASH, TOOLS_PRE_EXECUTE};

tokio::task_local! {
    static GRANTED: Access;
}

/// 在 `access` 档的沙箱授权下跑 `fut`（`None` = 不在沙箱里跑）。
pub(crate) async fn with_access<F: Future>(access: Option<Access>, fut: F) -> F::Output {
    match access {
        Some(access) => GRANTED.scope(access, fut).await,
        None => fut.await,
    }
}

/// 关卡给这次调用的沙箱档位。`None` = 不进沙箱（沙箱没开、用户批准了出沙箱、不在注册表里调）。
pub(crate) fn granted_access() -> Option<Access> {
    GRANTED.try_with(|a| *a).ok()
}

/// 这一页开没开沙箱。
pub fn enabled(page: &Context) -> bool {
    page.get::<AppSettings>(SETTINGS)
        .is_some_and(|s| s.sandbox())
}

/// `[sandbox]` 换成策略用的设置；`~/` 展开成家目录，相对路径不认（没有明确的基准）。
fn settings() -> Settings {
    let cfg = cordis_base::config::load_sandbox_config();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let paths = |raw: &[String]| -> Vec<PathBuf> {
        raw.iter()
            .filter_map(|s| expand_home(s.trim(), home.as_deref()))
            .collect()
    };
    Settings {
        network: cfg.network != SandboxNetwork::Off,
        proxy_only: cfg.network == SandboxNetwork::Proxy,
        build_caches: cfg.build_caches,
        writable_roots: paths(&cfg.writable_roots),
        deny_read: paths(&cfg.deny_read),
        keep_env: cfg.keep_env,
    }
}

fn expand_home(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    if raw == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

/// 这一页此刻的策略：可写根是这一页的沙箱工作区（不是 cwd，工作区里 `/cd` 到子目录照样
/// 写得了整个项目）；工作树还能写原仓库的 git 目录。
pub fn policy(page: &Context, access: Access) -> Policy {
    let space = crate::session::cwd::session_space(page);
    Policy::for_space(access, &space, &settings(), &host())
}

fn host() -> Host {
    let host = Host::detect(cordis_base::config::dock_home());
    // 测试的工作区建在临时目录里：临时目录照常放行的话，「工作区外写不了」「只读沙箱里写
    // 不了」都测不出来。
    #[cfg(test)]
    let host = Host {
        tmp_dirs: Vec::new(),
        ..host
    };
    host
}

/// 注册表对一次调用的沙箱意见。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// 沙箱没开、后端不可用、或和这颗工具无关：照原来的关卡走。
    Default,
    /// 命令在沙箱里按这一档跑：不再过权限门、计划门和只读关卡。
    Sandboxed(Access),
    /// 写在沙箱可写范围内：不弹权限框（计划门照旧）。
    SkipPrompt,
    /// 读的是拒读名单里的路径：本来不过门的读工具也要问。
    Ask,
}

/// 跑命令的工具。
fn runs_commands(tool: &str) -> bool {
    matches!(tool, "bash" | "run_terminal_cmd" | "monitor")
}

pub(crate) fn gate(exec: &Context, tool: &str, arguments: &str) -> Gate {
    // 限制默认不开：沙箱开着也只是审计 + 策略，关卡照旧。
    if !enabled(exec) || !cordis_base::config::load_sandbox_config().confine {
        return Gate::Default;
    }
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let path_arg = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .map(|raw| resolve(exec, raw))
    };
    match tool {
        t if runs_commands(t) => {
            if escapes(&args) || !dock_sandbox::availability().is_ready() {
                return Gate::Default;
            }
            let access = if crate::tools::read_only::in_read_only_scene(exec) {
                Access::ReadOnly
            } else {
                Access::WorkspaceWrite
            };
            Gate::Sandboxed(access)
        }
        "write_file" | "search_replace" => match path_arg(&["target_file", "file_path", "path"]) {
            Some(path) if policy(exec, Access::WorkspaceWrite).allows_write(&path) => {
                Gate::SkipPrompt
            }
            _ => Gate::Default,
        },
        "read_file" | "list_dir" | "grep" | "glob" => {
            let path = path_arg(&["target_file", "target_directory", "path"])
                .unwrap_or_else(|| crate::session::cwd::session_cwd(exec));
            if policy(exec, Access::WorkspaceWrite).denies_read(&path) {
                Gate::Ask
            } else {
                Gate::Default
            }
        }
        _ => Gate::Default,
    }
}

/// 模型要这条命令出沙箱跑（`"sandbox": false`）。
pub(crate) fn escapes(args: &Value) -> bool {
    args.get("sandbox").and_then(Value::as_bool) == Some(false)
}

fn resolve(exec: &Context, raw: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(path) = expand_home(raw, home.as_deref()) {
        return path;
    }
    crate::session::cwd::session_cwd(exec).join(raw)
}

/// 要在沙箱里跑的一条命令：策略在关卡那一刻定好，真正包装在 `Jobs` 起进程时做
/// （[`SandboxRun::prepare`]）——审计要先在命令前补上 `export`，沙箱规则也要看出口代理的口。
pub struct SandboxRun {
    pub(crate) policy: Policy,
    pub(crate) network: bool,
}

impl SandboxRun {
    /// 把最终要跑的 `command`（`bash -lc` 跑）包进沙箱。包不上返回原因，调用方**不跑**。
    pub(crate) fn prepare(&self, command: &str) -> Result<dock_sandbox::Prepared, String> {
        let argv = ["bash", "-lc", command].map(std::ffi::OsString::from);
        dock_sandbox::prepare(&self.policy, &argv)
            .map_err(|e| format!("Error: 沙箱包不上这条命令，没有运行：{e}"))
    }
}

/// 按关卡给的档位定这条命令的沙箱策略。沙箱后端不可用时当场报给模型。
pub(crate) fn prepare_command(access: Access, _command: &str) -> Result<SandboxRun, String> {
    if let Availability::Unavailable { reason } = dock_sandbox::availability() {
        return Err(format!("Error: 沙箱不可用，没有运行：{reason}"));
    }
    let page = crate::tools::registry::exec_ctx();
    let policy = match page.as_ref() {
        Some(page) => policy(page, access),
        None => Policy::build(
            access,
            &crate::session::cwd::current_cwd(),
            &settings(),
            &host(),
        ),
    };
    Ok(SandboxRun {
        network: policy.network(),
        policy,
    })
}

/// 命令像是被沙箱拦下时，接在输出后面给模型的话。
pub(crate) fn denial_hint(network: bool) -> String {
    let net = if network { "" } else { "，也不能联网" };
    format!(
        "[沙箱] 这条命令像是被沙箱拦下了：沙箱里只能写工作区、临时目录和依赖缓存，\
         读不到密钥（~/.ssh 等），不能改 .git/hooks、.git/config、.dock{net}。\
         确实需要越过沙箱，就把同一条命令加上 \"sandbox\": false 重跑，会先请用户批准；\
         不要换写法绕开。"
    )
}

/// Landlock 那边事后比对的结果，接在输出后面。
pub(crate) fn guard_note(report: &dock_sandbox::GuardReport) -> Option<String> {
    if report.is_clean() {
        return None;
    }
    let list = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("、")
    };
    let mut lines = Vec::new();
    if !report.restored.is_empty() {
        lines.push(format!(
            "[沙箱] 命令改了受保护的路径，已还原：{}（沙箱里不能改 .git/hooks、.git/config、.dock）",
            list(&report.restored)
        ));
    }
    if !report.unrestored.is_empty() {
        lines.push(format!(
            "[沙箱] 受保护的路径被改了、没能还原，请检查：{}",
            list(&report.unrestored)
        ));
    }
    Some(lines.join("\n"))
}

/// 跑命令的工具，和它们的命令参数。
fn command_arg(tool: &str, args: &Value) -> Option<String> {
    runs_commands(tool)
        .then(|| {
            args.get("command")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .flatten()
}

/// 带网址的工具（Dock 自己出网、浏览器跳转）和它们的网址里的主机。
fn url_host(tool: &str, args: &Value) -> Option<String> {
    let url = match tool {
        "web_fetch" | "mcp_browser__browser_open" | "mcp_browser__browser_navigate" => {
            args.get("url").and_then(Value::as_str)?
        }
        _ => return None,
    };
    // 按 URL 规范解析：百分号编码（`foo.corp%2eexample`）、大小写、IDN 都规范成主机真正的名字，
    // 规则才不会被换个写法绕开。没写协议的（`example.com/x`）按 http 补上。
    let parsed = url::Url::parse(url)
        .ok()
        .filter(|u| u.host_str().is_some())
        .or_else(|| url::Url::parse(&format!("http://{url}")).ok())?;
    let host = parsed.host_str()?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// 沙箱策略落在一次调用上：命令按命令规则、带网址的按主机规则。阻断 = 拒掉这次调用；
/// 询问 = 要人点头（自动批准不算数）。沙箱没开、没命中规则都不动它（默认放行）。
fn on_policy(pre: &mut PreExecute) {
    let Some(exec) = crate::tools::registry::exec_ctx() else {
        return;
    };
    if !enabled(&exec) {
        return;
    }
    let Some(audit) = exec.get::<Audit>(AUDIT) else {
        return;
    };
    let args: Value = serde_json::from_str(pre.arguments()).unwrap_or(Value::Null);
    let rules = audit.rules_for(&exec);
    let (rule, what) = if let Some(command) = command_arg(pre.name(), &args) {
        (rules::for_command(&rules, &command).cloned(), "命令")
    } else if let Some(host) = url_host(pre.name(), &args) {
        (rules::for_host(&rules, &host).cloned(), "网址")
    } else {
        return;
    };
    let Some(rule) = rule else {
        return;
    };
    let pattern = rule.command.or(rule.host).unwrap_or_default();
    match rule.action {
        Action::Allow => {}
        Action::Deny => pre.deny(format!(
            "沙箱策略阻断了这个{what}（规则 {pattern} → deny）。不要换写法绕开；确实需要就请用户 /sandbox allow 放行。"
        )),
        Action::Ask => pre.ask(format!("沙箱策略：{what}命中规则 {pattern} → ask")),
    }
}

/// 这一页的工作区，给人看。
fn workspace_lines(page: &Context) -> Vec<String> {
    let space = crate::session::cwd::session_space(page);
    let cwd = crate::session::cwd::session_cwd(page);
    let mut out = vec![format!(
        "工作区：{}（{}）",
        space.root.display(),
        space.describe_mode()
    )];
    if space.root != space.home {
        out.push(format!(
            "  项目：{}（会话归在它名下）",
            space.home.display()
        ));
    }
    if cwd != space.root {
        out.push(format!("  当前目录：{}", cwd.display()));
    }
    if let Some(branch) = space.branch() {
        out.push(format!(
            "  合回去：在项目里 git merge {branch}；不要了：/sandbox inplace 之后 git worktree remove {}",
            space.root.display()
        ));
    }
    out
}

/// 这一页正在跑一轮（或有排队的）：这时换工作区，跑到一半的命令和接下来的调用就不在同一个地方了。
fn page_busy(page: &Context) -> bool {
    page.get::<crate::host::session_port::SessionRef>(crate::names::SESSION_PORT)
        .is_some_and(|port| port.working() || port.has_queued())
}

/// 换了工作区：给模型留一句（下一轮看得到），免得它照着之前的路径改回原项目。
fn tell_model(page: &Context, text: String) {
    if let Some(sessions) = page.get::<crate::session::log::Sessions>(crate::names::SESSIONS) {
        sessions.append(cordis_base::types::LogEvent::SystemReminder(text));
    }
}

/// `/sandbox worktree`：从项目仓库建一棵工作树，这一页换进去。
fn enter_worktree(page: &Context) -> String {
    let space = crate::session::cwd::session_space(page);
    if let Some(branch) = space.branch() {
        return format!(
            "这一页已经在工作树里：{}（分支 {branch}）",
            space.root.display()
        );
    }
    if page_busy(page) {
        return "这一页正在跑，等这一轮结束再换工作区".into();
    }
    let id: String = crate::session::persist::new_id()
        .chars()
        .rev()
        .take(8)
        .collect();
    let store = cordis_base::config::dock_home().join(dock_sandbox::space::STORE_DIR);
    let tree = match dock_sandbox::space::create_worktree(&space.home, &store, &id) {
        Ok(tree) => tree,
        Err(e) => return e,
    };
    let branch = tree.branch().unwrap_or_default().to_string();
    let (root, home) = (tree.root.clone(), tree.home.clone());
    let cwd = match crate::session::cwd::switch_space(page, tree) {
        Ok(cwd) => cwd,
        Err(e) => return e,
    };
    tell_model(
        page,
        format!(
            "[沙箱] 这一页的工作区换成了工作树 {}（分支 {branch}，从 HEAD 检出）。之后在这里读写、跑命令；\
             原项目 {} 里的文件不要再改。之前提到的 {} 下的路径，对应到 {} 下的同一位置。当前目录：{}",
            root.display(),
            home.display(),
            home.display(),
            root.display(),
            cwd.display()
        ),
    );
    persist(page);
    format!(
        "这一页换进了工作树 {}（分支 {branch}，从 HEAD 检出，没提交的改动不带过去）。改动不碰 {}；\
         /sandbox inplace 回原项目。",
        root.display(),
        home.display()
    )
}

/// `/sandbox inplace`：回到原项目（工作树留着）。
fn leave_worktree(page: &Context) -> String {
    let space = crate::session::cwd::session_space(page);
    let Some(branch) = space.branch().map(str::to_string) else {
        return format!("这一页已经在原地工作区：{}", space.root.display());
    };
    if page_busy(page) {
        return "这一页正在跑，等这一轮结束再换工作区".into();
    }
    let home = space.home.clone();
    if let Err(e) =
        crate::session::cwd::switch_space(page, dock_sandbox::space::Space::in_place(&home))
    {
        return e;
    }
    tell_model(
        page,
        format!(
            "[沙箱] 这一页回到了原项目 {}（原地）。工作树 {} 里的改动没有合进来（分支 {branch}）。",
            home.display(),
            space.root.display()
        ),
    );
    persist(page);
    format!(
        "回到原项目 {}。工作树 {} 和分支 {branch} 留着：合回去 git merge {branch}，不要了 git worktree remove {}",
        home.display(),
        space.root.display(),
        space.root.display()
    )
}

/// 位置变了马上写进 `meta.json`：换完就关掉的会话，下次打开还在换过的地方。
fn persist(page: &Context) {
    if let Some(sessions) = page.get::<crate::session::log::Sessions>(crate::names::SESSIONS) {
        sessions.persist_place();
    }
}

/// `/sandbox passthrough [clear [主机]]`：看 / 清直通名单（客户端不信 Dock 根证书、原样转发的主机）。
fn passthrough_command(audit: &Audit, rest: &str) -> SlashOutcome {
    let (verb, host) = match rest.split_once(char::is_whitespace) {
        Some((verb, host)) => (verb, Some(host.trim()).filter(|h| !h.is_empty())),
        None => (rest, None),
    };
    match verb {
        "" | "list" => {
            let hosts = audit.passthrough();
            if hosts.is_empty() {
                return SlashOutcome::Applied("直通名单是空的：命令的 HTTPS 都在解密".into());
            }
            let lines: Vec<String> = hosts
                .iter()
                .map(|(h, saved)| {
                    if *saved {
                        format!("  {h}（确定被拒，30 天内都原样转发）")
                    } else {
                        format!("  {h}（这次运行里学到的，重启后再试着解密）")
                    }
                })
                .collect();
            SlashOutcome::notice(
                "直通名单",
                format!(
                    "这些主机的 HTTPS 原样转发、不解密（客户端不信 Dock 根证书）：\n{}\n/sandbox passthrough clear [主机] 清掉",
                    lines.join("\n")
                ),
            )
        }
        "clear" | "清" => {
            let n = audit.forget_passthrough(host);
            SlashOutcome::Applied(match host {
                Some(h) if n > 0 => format!("{h} 从直通名单里拿掉了，之后又解密"),
                Some(h) => format!("{h} 不在直通名单里"),
                None => format!("直通名单清空了（{n} 个主机），之后又解密"),
            })
        }
        other => SlashOutcome::notice(
            "沙箱",
            format!("不认识「{other}」：用 /sandbox passthrough [clear [主机]]"),
        ),
    }
}

/// 给人看的现状。
fn describe(page: &Context) -> String {
    let cfg = cordis_base::config::load_sandbox_config();
    let on = enabled(page);
    let mut out = vec![format!("这一页的沙箱：{}", if on { "开" } else { "关" })];
    out.extend(workspace_lines(page));
    if let Some(audit) = page.get::<Audit>(AUDIT) {
        out.extend(crate::host::audit::status_lines(&audit, page));
        let rules = audit.rules_for(page);
        if rules.is_empty() {
            out.push(
                "策略：没有规则，全部放行（[[sandbox.rules]] 加规则，/sandbox allow 临时放行）"
                    .into(),
            );
        } else {
            out.push("策略（按顺序第一条命中的算，都不中就放行）：".into());
            for r in &rules {
                let what = match (&r.host, &r.command) {
                    (Some(h), _) => format!("出网 {h}"),
                    (_, Some(c)) => format!("命令 {c}"),
                    _ => continue,
                };
                let action = match r.action {
                    Action::Allow => "放行",
                    Action::Ask => "询问",
                    Action::Deny => "阻断",
                };
                out.push(format!("  {what} → {action}"));
            }
        }
    }
    if !cfg.confine {
        out.push(
            "限制：没开（命令照常跑、照常过权限门；[sandbox] confine = true 让命令进系统沙箱）"
                .into(),
        );
        return out.join("\n");
    }
    match dock_sandbox::availability() {
        Availability::Unavailable { reason } => {
            out.push(format!("限制：本机不可用（{reason}），命令照常走权限确认"));
        }
        Availability::Ready { backend, limits } => {
            out.push(format!("限制：命令在系统沙箱里跑（{}）", backend.name()));
            let net = match cfg.network {
                SandboxNetwork::Allow => "能联网",
                SandboxNetwork::Proxy if cfg!(target_os = "macos") => {
                    "只能经审计代理出网（直连连不上）"
                }
                SandboxNetwork::Proxy => "只能经审计代理出网（这个系统上内核挡不住直连）",
                SandboxNetwork::Off => "不能联网",
            };
            out.push(format!("  网络：{net}"));
            out.push("  可写：工作区（工作树还有原仓库的 .git）、临时目录、$DOCK_HOME/scratch，以及依赖缓存和 [sandbox] writable_roots".into());
            out.push(
                "  不可读：~/.ssh、~/.aws、钥匙串、Dock 的密钥等（[sandbox] deny_read 可加）"
                    .into(),
            );
            out.push("  只读：.git/hooks、.git/config、.dock".into());
            for limit in limits {
                out.push(format!("  限制：{}", limit.describe()));
            }
            if on {
                out.push(
                    "  沙箱里的命令不再弹权限框；要出沙箱的（\"sandbox\": false）照常问你。".into(),
                );
            }
        }
    }
    out.join("\n")
}

fn sandbox_command(page: &Context, args: &str) -> SlashOutcome {
    let Some(settings) = page.get::<AppSettings>(SETTINGS) else {
        return SlashOutcome::notice("沙箱", "设置服务未挂载。");
    };
    let audit = page.get::<Audit>(AUDIT);
    let (sub, rest) = match args.trim().split_once(char::is_whitespace) {
        Some((sub, rest)) => (sub, rest.trim()),
        None => (args.trim(), ""),
    };
    let turn = |on: bool| {
        settings.set_sandbox(on);
        if !on {
            if let Some(audit) = &audit {
                audit.close(page);
            }
        }
        // 记进 meta.json：Dock 重启后打开这个会话，沙箱还是这个状态。
        if let Some(sessions) = page.get::<crate::session::log::Sessions>(crate::names::SESSIONS) {
            sessions.persist_sandbox();
        }
    };
    match sub {
        "" => turn(!settings.sandbox()),
        "on" | "开" => turn(true),
        "off" | "关" => turn(false),
        "status" | "状态" => {}
        "passthrough" | "直通" => {
            let Some(audit) = audit else {
                return SlashOutcome::notice("沙箱", "审计服务未挂载。");
            };
            return passthrough_command(&audit, rest);
        }
        "worktree" | "工作树" => return SlashOutcome::Applied(enter_worktree(page)),
        "inplace" | "原地" => return SlashOutcome::Applied(leave_worktree(page)),
        "graph" | "图" => {
            return match crate::host::audit::graph_text(page) {
                Some(text) => SlashOutcome::notice("沙箱链路图", text),
                None => SlashOutcome::Applied("这一页的会话还没有审计记录（/sandbox on 之后开始记）".into()),
            }
        }
        "export" | "导出" => {
            return match audit.as_ref().and_then(|a| a.active(page)) {
                Some(active) => match crate::host::audit::export(&active) {
                    Ok(report) => SlashOutcome::Applied(format!("已导出：{}", report.display())),
                    Err(e) => SlashOutcome::Applied(format!("导出失败：{e}")),
                },
                None => SlashOutcome::Applied("这一页的沙箱没开（/sandbox on）".into()),
            }
        }
        "allow" | "allow-cmd" => {
            let Some(audit) = audit else {
                return SlashOutcome::notice("沙箱", "审计服务未挂载。");
            };
            if !settings.sandbox() {
                return SlashOutcome::Applied("这一页的沙箱没开（/sandbox on）".into());
            }
            if rest.is_empty() {
                return SlashOutcome::Applied("用法：/sandbox allow <域名通配>，/sandbox allow-cmd <命令通配>".into());
            }
            let rule = if sub == "allow" {
                Rule { host: Some(rest.to_string()), command: None, action: Action::Allow }
            } else {
                Rule { host: None, command: Some(rest.to_string()), action: Action::Allow }
            };
            audit.grant(page, rule);
            return SlashOutcome::Applied(format!("这次任务里放行：{rest}（沙箱关掉就作废）"));
        }
        other => {
            return SlashOutcome::notice(
                "沙箱",
                format!("不认识「{other}」：用 /sandbox [on|off|status|graph|export|allow|allow-cmd|passthrough|worktree|inplace]，不带参数是切换。"),
            )
        }
    }
    SlashOutcome::notice("沙箱", describe(page))
}

/// `sandbox` 插件：播种第 1 页的开关、挂 `/sandbox`（别名 `/audit`），在 `tools/pre-execute`
/// 上落沙箱策略。
pub fn sandbox() -> Plugin {
    plugin("sandbox", Inject::from([SLASH]), |ctx, _: &()| {
        if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
            settings.set_sandbox(cordis_base::config::load_sandbox_config().enabled);
        }
        register_commands(
            ctx,
            vec![SlashCommand::host(
                "sandbox",
                "沙箱：审计这一页的工具调用、命令和出网，按策略放行 / 询问 / 阻断",
                slash_handler(sandbox_command),
            )
            .aliases(&["audit"])
            .takes_args(true)
            .hint("sandbox on", "开这一页的沙箱（审计 + 策略）", false)
            .hint("sandbox off", "关这一页的沙箱", false)
            .hint("sandbox graph", "看这一页的行为链路图", false)
            .hint("sandbox export", "导出审计报告（HTML + 原始记录）", false)
            .hint("sandbox allow", "这次任务里放行一个域名（通配）", true)
            .hint("sandbox allow-cmd", "这次任务里放行一类命令（通配）", true)
            .hint(
                "sandbox passthrough",
                "看 / 清直通名单（不解密的主机）",
                true,
            )
            .hint(
                "sandbox worktree",
                "这一页换进项目的一棵 git 工作树（改动不碰原项目）",
                false,
            )
            .hint("sandbox inplace", "这一页回到原项目（工作树留着）", false)],
        )?;
        let policy = ctx
            .clone()
            .on_waterfall(TOOLS_PRE_EXECUTE, |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                if next.denial().is_none() {
                    on_policy(&mut next);
                }
                next
            })?;
        let all = ctx.effect("sandbox", |scope| {
            scope.own(policy);
            Ok(())
        })?;
        Ok(Some(all))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 沙箱开着、限制没开（默认）：一页只有审计和策略，配了 `rules` 的临时 `$DOCK_HOME`。
    async fn policy_page(config: &str) -> (cordis_base::test_env::EnvScope, Context) {
        let env = cordis_base::test_env::scoped().home();
        std::fs::write(cordis_base::config::dock_home().join("config.toml"), config).unwrap();
        let ctx = Context::new();
        for p in [
            crate::session::log::sessions(),
            crate::host::settings::settings(),
            crate::host::audit::audit(),
        ] {
            ctx.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        ctx.require::<AppSettings>(SETTINGS)
            .unwrap()
            .set_sandbox(true);
        (env, ctx)
    }

    fn pre(ctx: &Context, name: &str, args: serde_json::Value) -> PreExecute {
        let mut pre = PreExecute::new(name, args.to_string(), "main");
        crate::tools::registry::with_exec_ctx(ctx, || on_policy(&mut pre));
        pre
    }

    const RULES: &str = r#"[sandbox]
[[sandbox.rules]]
command = "git push*"
action = "ask"
[[sandbox.rules]]
command = "*rm -rf /*"
action = "deny"
[[sandbox.rules]]
host = "*.blocked.example"
action = "deny"
"#;

    /// 沙箱开着、限制没开：关卡照旧（不进系统沙箱、也不替权限档位放行），策略照常落。
    #[tokio::test]
    async fn without_confine_the_gate_stays_default_and_rules_apply() {
        let (_env, ctx) = policy_page(RULES).await;
        assert_eq!(
            gate(&ctx, "bash", r#"{"command":"ls"}"#),
            Gate::Default,
            "沙箱开着但没开限制：照常过权限门"
        );
        let push = pre(
            &ctx,
            "bash",
            serde_json::json!({"command": "git push origin main"}),
        );
        assert!(
            push.ask_reason().unwrap().contains("git push*"),
            "{:?}",
            push.ask_reason()
        );
        let rm = pre(
            &ctx,
            "bash",
            serde_json::json!({"command": "cd x && rm -rf /"}),
        );
        assert!(rm.denial().unwrap().contains("deny"), "{:?}", rm.denial());
        let fine = pre(&ctx, "bash", serde_json::json!({"command": "cargo build"}));
        assert!(
            fine.denial().is_none() && fine.ask_reason().is_none(),
            "默认放行"
        );
        let fetch = pre(
            &ctx,
            "web_fetch",
            serde_json::json!({"url": "https://api.blocked.example/x"}),
        );
        assert!(fetch.denial().is_some(), "网址也按主机规则判");
        let nav = pre(
            &ctx,
            "mcp_browser__browser_navigate",
            serde_json::json!({"url": "https://www.example.org/"}),
        );
        assert!(nav.denial().is_none());
    }

    /// dock.1 `sandbox/status` / `sandbox/trace`：状态里有开关、工作区、规则；记下的工具调用出现在
    /// 时间线和链路图里；关掉沙箱就不在记了。
    #[tokio::test]
    async fn status_and_trace_describe_the_page() {
        let (_env, ctx) = policy_page(RULES).await;
        let status = crate::host::audit::sandbox_status(&ctx);
        assert_eq!(status["enabled"], true);
        assert_eq!(status["auditing"], true);
        assert_eq!(status["workspace"]["mode"], "in_place");
        assert_eq!(status["rules"].as_array().unwrap().len(), 3, "{status}");
        assert_eq!(status["rules"][0]["action"], "ask");
        // 后端要么能用（带做不到的保证），要么不能用（带原因），设置页照着写。
        assert_ne!(
            status["backend"].is_null(),
            status["backendReason"].is_null(),
            "{status}"
        );
        assert!(status["limits"].is_array(), "{status}");

        let audit = ctx.get::<Audit>(AUDIT).unwrap();
        let active = audit.active(&ctx).expect("沙箱开着就在记");
        active.log.record(
            "tool_call",
            serde_json::json!({ "tool": "bash", "call_id": "c1", "args": "ls -la" }),
        );
        // 写盘在单独的线程上：等它写到。
        let mut trace = Value::Null;
        for _ in 0..100 {
            trace = crate::host::audit::sandbox_trace(&ctx, 50);
            if trace["timeline"]
                .as_array()
                .is_some_and(|t| t.iter().any(|r| r["kind"] == "tool"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(trace["auditing"], true);
        let tool = trace["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "tool")
            .cloned()
            .unwrap_or_else(|| panic!("{trace}"));
        assert_eq!(
            (tool["title"].clone(), tool["call"].clone()),
            ("bash".into(), "c1".into())
        );
        assert!(
            trace["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == "tool:bash"),
            "{trace}"
        );
        assert_eq!(trace["totals"]["toolCalls"], 1);

        // HTTP 历史：一次出网列出来，按 id 取回请求包 / 响应包。
        use dock_sandbox::audit::record::{Captured, Flow, Message};
        active.log.flow(Flow {
            tag: "job-1".into(),
            kind: "http",
            scheme: "https",
            host: "api.example.com".into(),
            port: 443,
            method: Some("GET".into()),
            path: Some("/v1/items".into()),
            status: Some(200),
            request: Some(Message {
                headers: vec![("accept".into(), "application/json".into())],
                body: Captured::default(),
            }),
            response: Some(Message {
                headers: vec![("content-type".into(), "application/json".into())],
                body: Captured {
                    size: 11,
                    truncated: false,
                    sha256: None,
                    sample: br#"{"ok":true}"#.to_vec(),
                },
            }),
            bytes_up: 10,
            bytes_down: 11,
            started_ms: 1,
            duration_ms: 5,
            decrypted: true,
            note: None,
            error: None,
        });
        let mut http = Value::Null;
        for _ in 0..100 {
            http = crate::host::audit::sandbox_http(&ctx, 100);
            if http["requests"].as_array().is_some_and(|r| !r.is_empty()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let req = &http["requests"][0];
        assert_eq!(
            (req["method"].clone(), req["status"].clone()),
            ("GET".into(), 200.into()),
            "{http}"
        );
        let id = req["id"].as_str().expect("带 id");
        let flow = crate::host::audit::sandbox_flow(&ctx, id).expect("按 id 取得回");
        assert_eq!(flow["request"]["line"], "GET /v1/items HTTP/1.1");
        assert_eq!(flow["response"]["line"], "HTTP/1.1 200 OK");
        assert_eq!(flow["response"]["body"]["text"], r#"{"ok":true}"#);

        sandbox_command(&ctx, "off");
        assert_eq!(
            crate::host::audit::sandbox_trace(&ctx, 50)["auditing"],
            false
        );
    }

    /// 回归：Dock 重启后打开老会话，之前几次运行记下的要照样在行为轨迹 / HTTP 历史里、
    /// 按 id 取得回包。以前只读这一次运行新开的目录，重启前的全看不到。
    /// 两次运行都有 `job-1`：前一次的出网不能算到这一次的命令头上。别的会话的目录不混进来，
    /// id 里的目录名也只认这个会话自己的。
    #[tokio::test]
    async fn history_spans_dock_restarts_of_the_same_session() {
        use dock_sandbox::audit::record::{Captured, Flow, Message};
        use dock_sandbox::audit::store::AuditLog;
        let (_env, ctx) = policy_page(RULES).await;
        let audit = ctx.get::<Audit>(AUDIT).unwrap();
        // 这一次运行：先开出来，好知道这一页的会话在目录名里叫什么。
        let now = audit.active(&ctx).expect("沙箱开着就在记");
        let name = now.dir.file_name().unwrap().to_string_lossy().into_owned();
        let suffix = &name[15..]; // `-<会话>`
        let root = cordis_base::config::dock_home().join("audit");

        // 上一次运行（Dock 重启前）：同一个会话，job-1 = curl，发过一次出网。
        let old = root.join(format!("20200101-000000{suffix}"));
        let log = AuditLog::open(&old, "events.jsonl").unwrap();
        log.record(
            "command",
            serde_json::json!({ "job": "job-1", "tool": "bash", "program": "curl", "command": "curl https://old.example" }),
        );
        log.flow(Flow {
            tag: "job-1".into(),
            kind: "http",
            scheme: "https",
            host: "old.example".into(),
            port: 443,
            method: Some("GET".into()),
            path: Some("/before-restart".into()),
            status: Some(200),
            request: Some(Message {
                headers: vec![],
                body: Captured::default(),
            }),
            response: Some(Message {
                headers: vec![],
                body: Captured::default(),
            }),
            bytes_up: 1,
            bytes_down: 1,
            started_ms: 1,
            duration_ms: 1,
            decrypted: true,
            note: None,
            error: None,
        });
        drop(log); // 写盘线程收尾
                   // 别的会话的目录：不该混进来。
        let other = root.join("20200101-000000-someone_else");
        let log = AuditLog::open(&other, "events.jsonl").unwrap();
        log.record(
            "tool_call",
            serde_json::json!({ "tool": "rm", "call_id": "x" }),
        );
        drop(log);

        // 这一次：job-1 是 git。
        now.log.record(
            "command",
            serde_json::json!({ "job": "job-1", "tool": "bash", "program": "git", "command": "git status" }),
        );
        let mut trace = Value::Null;
        for _ in 0..100 {
            trace = crate::host::audit::sandbox_trace(&ctx, 50);
            if trace["timeline"]
                .as_array()
                .is_some_and(|t| t.iter().any(|r| r["title"] == "git"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(trace["runs"], 2, "{trace}");
        let rows = trace["timeline"].as_array().unwrap();
        let jobs: Vec<(String, String)> = rows
            .iter()
            .filter(|r| r["kind"] == "command")
            .map(|r| {
                (
                    r["title"].as_str().unwrap().into(),
                    r["job"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert_eq!(
            jobs,
            [
                ("curl".into(), "job-1@1".into()),
                ("git".into(), "job-1".into())
            ],
            "{trace}"
        );
        assert!(
            !trace.to_string().contains("\"rm\""),
            "别的会话混进来了：{trace}"
        );

        // HTTP 历史里有重启前的那次，按 id 取得回包。
        let http = crate::host::audit::sandbox_http(&ctx, 100);
        let req = http["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["host"] == "old.example")
            .cloned()
            .unwrap_or_else(|| panic!("{http}"));
        assert_eq!(req["tag"], "job-1@1");
        let id = req["id"].as_str().unwrap();
        assert!(id.starts_with("20200101-000000"), "{id}");
        let flow = crate::host::audit::sandbox_flow(&ctx, id).expect("重启前的包取得回");
        assert_eq!(flow["request"]["line"], "GET /before-restart HTTP/1.1");
        assert_eq!(flow["id"], id);
        for bad in [
            "20200101-000000-someone_else/events.jsonl:1",
            "../x/events.jsonl:1",
        ] {
            assert!(
                crate::host::audit::sandbox_flow(&ctx, bad).is_none(),
                "{bad}"
            );
        }

        // 沙箱关了：之前记的照样看得到，只是不再接着记。
        sandbox_command(&ctx, "off");
        let trace = crate::host::audit::sandbox_trace(&ctx, 50);
        assert_eq!(trace["auditing"], false);
        assert_eq!(trace["runs"], 2);
        assert!(trace["records"].as_u64().unwrap() >= 3, "{trace}");
    }

    /// 一页换了会话（`/resume`、开了别的）：前一个会话的审计记「结束」，新会话另开一份目录，
    /// 前一个会话的临时放行（`/sandbox allow`）也不再生效。以前都按页缓存：换了会话还往前一个
    /// 会话的目录里记，放行也带了过去。
    #[tokio::test]
    async fn a_page_that_switches_sessions_starts_a_new_audit() {
        let (_env, ctx) = policy_page(RULES).await;
        let audit = ctx.get::<Audit>(AUDIT).unwrap();
        let first = audit.active(&ctx).unwrap();
        // 这个会话里临时放行一类命令。
        sandbox_command(&ctx, "allow-cmd git push*");
        let granted = |rules: Vec<Rule>| {
            rules
                .iter()
                .any(|r| r.command.as_deref() == Some("git push*") && r.action == Action::Allow)
        };
        assert!(granted(audit.rules_for(&ctx)));
        // 这一页落盘、有了会话 id：算换了一个会话。
        let sessions = ctx
            .require::<crate::session::log::Sessions>(crate::names::SESSIONS)
            .unwrap();
        sessions.attach_disk();
        sessions.append(cordis_base::types::LogEvent::User("hi".into()));
        let id = sessions.live_session_id();
        assert!(!id.is_empty());
        // 换了会话：上一个会话的临时放行不再生效（不管审计这边先没先发现换了会话）。
        assert!(
            !granted(audit.rules_for(&ctx)),
            "上一个会话的 /sandbox allow 不该带到新会话"
        );
        let second = audit.active(&ctx).unwrap();
        assert_ne!(first.dir, second.dir);
        assert!(
            second.dir.to_string_lossy().ends_with(&id),
            "{}",
            second.dir.display()
        );
        assert!(
            std::sync::Arc::ptr_eq(&second, &audit.active(&ctx).unwrap()),
            "同一个会话沿用"
        );
        let first_dir = first.dir.clone();
        drop(first);
        let mut ended = false;
        for _ in 0..100 {
            let events = dock_sandbox::audit::store::read(&first_dir);
            if events.iter().any(|e| e["event"] == "结束") {
                ended = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(ended, "前一个会话的审计要记「结束」");
    }

    /// 这次任务临时放行的排在配置规则前面；沙箱关了就作废。沙箱关着时策略不落。
    #[tokio::test]
    async fn task_grants_override_config_rules_until_the_sandbox_closes() {
        let (_env, ctx) = policy_page(RULES).await;
        let out = sandbox_command(&ctx, "allow-cmd git push origin main");
        assert!(
            matches!(out, SlashOutcome::Applied(ref t) if t.contains("放行")),
            "{out:?}"
        );
        let push = pre(
            &ctx,
            "bash",
            serde_json::json!({"command": "git push origin main"}),
        );
        assert!(push.ask_reason().is_none(), "临时放行盖过配置的 ask");
        assert!(
            crate::host::audit::check_host(&ctx, "x.blocked.example", Some(443))
                .await
                .is_some(),
            "出口代理按同一套规则阻断"
        );
        sandbox_command(&ctx, "allow x.blocked.example");
        assert!(
            crate::host::audit::check_host(&ctx, "x.blocked.example", Some(443))
                .await
                .is_none()
        );

        sandbox_command(&ctx, "off");
        let push = pre(&ctx, "bash", serde_json::json!({"command": "rm -rf /"}));
        assert!(push.denial().is_none(), "沙箱关着：策略不落");
        sandbox_command(&ctx, "on");
        let push = pre(
            &ctx,
            "bash",
            serde_json::json!({"command": "git push origin main"}),
        );
        assert!(push.ask_reason().is_some(), "重开之后，上次的临时放行作废");
    }

    /// `network = "proxy"`：限制开着时命令只能连本机（出网只能经审计代理）。
    #[tokio::test]
    async fn proxy_network_reaches_the_command_policy() {
        let (_env, ctx) = policy_page("[sandbox]\nconfine = true\nnetwork = \"proxy\"\n").await;
        let p = policy(&ctx, Access::WorkspaceWrite);
        assert!(p.network() && p.proxy_only());
        std::fs::write(
            cordis_base::config::dock_home().join("config.toml"),
            "[sandbox]\nconfine = true\nnetwork = \"off\"\n",
        )
        .unwrap();
        assert!(!policy(&ctx, Access::WorkspaceWrite).network());
    }

    #[test]
    fn home_expansion_only_takes_absolute_or_tilde() {
        let home = Path::new("/home/u");
        assert_eq!(
            expand_home("~/build", Some(home)),
            Some(PathBuf::from("/home/u/build"))
        );
        assert_eq!(expand_home("~", Some(home)), Some(home.to_path_buf()));
        assert_eq!(expand_home("/abs", Some(home)), Some(PathBuf::from("/abs")));
        assert_eq!(expand_home("relative", Some(home)), None);
        assert_eq!(expand_home("~/x", None), None);
    }

    /// 回归：网址按 URL 规范解析。以前手工切字符串、不解百分号编码，`foo.corp%2eexample`
    /// 绕得开 `*.corp.example` 的 deny。
    #[test]
    fn url_hosts_are_normalized_before_the_rules_see_them() {
        let host = |u: &str| url_host("web_fetch", &serde_json::json!({ "url": u }));
        assert_eq!(
            host("https://foo.corp%2eexample/x").as_deref(),
            Some("foo.corp.example")
        );
        assert_eq!(
            host("HTTPS://User:pw@API.Example.COM:8443/p?q#f").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(host("http://[::1]:3000/").as_deref(), Some("::1"));
        assert_eq!(host("example.org/path").as_deref(), Some("example.org"));
        assert_eq!(host("").as_deref(), None);
        let rules = vec![Rule {
            host: Some("*.corp.example".into()),
            command: None,
            action: Action::Deny,
        }];
        let h = host("https://foo.corp%2eexample/").unwrap();
        assert!(
            rules::for_host(&rules, &h).is_some(),
            "编码过的主机名也要命中 deny"
        );
    }

    #[test]
    fn escapes_only_on_an_explicit_false() {
        assert!(escapes(&serde_json::json!({"sandbox": false})));
        assert!(!escapes(&serde_json::json!({"sandbox": true})));
        assert!(!escapes(&serde_json::json!({})));
    }

    use crate::host::permissions::Permissions;
    use crate::host::settings::PermissionMode;
    use crate::names::{PERMISSIONS, PLAN_MODE, SESSIONS};
    use crate::session::log::Sessions;
    use crate::tools::plan_mode::PlanMode;
    use crate::tools::registry::Tools;
    use cordis_base::acp::PermissionOptionKind;
    use cordis_base::types::{ToolCall, ToolResult};

    /// 一页：会话 cwd 钉在临时目录里的工作区，询问档，沙箱开着、限制开着（`confine = true`，
    /// 写在临时的 `$DOCK_HOME/config.toml` 里）。
    struct Page {
        _env: cordis_base::test_env::EnvScope,
        _dir: tempfile::TempDir,
        ctx: Context,
        ws: PathBuf,
        outside: PathBuf,
        tools: Tools,
    }

    fn page() -> Page {
        let env = cordis_base::test_env::scoped().home();
        std::fs::write(
            cordis_base::config::dock_home().join("config.toml"),
            "[sandbox]\nconfine = true\n",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = root.join("ws");
        let outside = root.join("outside");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let ctx = Context::new();
        let sessions = Sessions::new(ctx.clone());
        sessions.pin_workspace_cwd(&ws);
        std::mem::forget(ctx.provide(SESSIONS, sessions).unwrap());
        let settings = AppSettings::new("x");
        settings.set_permission_mode(PermissionMode::Ask);
        settings.set_sandbox(true);
        std::mem::forget(ctx.provide(SETTINGS, settings).unwrap());
        std::mem::forget(
            ctx.provide(PERMISSIONS, Permissions::new(ctx.clone()))
                .unwrap(),
        );
        let tools = Tools::workspace(ctx.clone());
        Page {
            _env: env,
            _dir: dir,
            ctx,
            ws,
            outside,
            tools,
        }
    }

    /// 回归：限制模式的可写根是工作区，不是 cwd。以前工作区里 `/cd sub` 之后，写项目根下的
    /// 文件就算「沙箱外」，要弹框；sub 里跑的命令也写不了项目的其它目录。
    #[tokio::test]
    async fn the_writable_root_is_the_workspace_not_the_cwd() {
        let p = page();
        std::fs::create_dir_all(p.ws.join("sub")).unwrap();
        let sessions = p.ctx.get::<Sessions>(SESSIONS).unwrap();
        sessions.pin_space(dock_sandbox::space::Space::in_place(&p.ws));
        sessions.pin_workspace_cwd(p.ws.join("sub"));
        let write = |path: PathBuf| serde_json::json!({ "target_file": path }).to_string();
        assert_eq!(
            gate(&p.ctx, "write_file", &write(p.ws.join("top.txt"))),
            Gate::SkipPrompt
        );
        assert_eq!(
            policy(&p.ctx, Access::WorkspaceWrite).workspace(),
            p.ws.as_path()
        );
        assert_eq!(
            gate(&p.ctx, "write_file", &write(p.outside.join("x.txt"))),
            Gate::Default
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// `/sandbox worktree`：这一页换进项目的一棵工作树，cwd 换到工作树里的同一位置，模型收到
    /// 一句提醒；限制模式下写工作树不弹框、写原项目要问。`/sandbox inplace` 回原项目，工作树留着。
    #[tokio::test]
    async fn worktree_mode_moves_the_page_into_a_checkout_and_back() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let p = page();
        std::fs::create_dir_all(p.ws.join("sub")).unwrap();
        std::fs::write(p.ws.join("sub/a.txt"), "a").unwrap();
        git(&p.ws, &["init", "--quiet"]);
        git(&p.ws, &["add", "."]);
        git(&p.ws, &["commit", "--quiet", "-m", "init"]);
        let sessions = p.ctx.get::<Sessions>(SESSIONS).unwrap();
        sessions.pin_space(dock_sandbox::space::Space::in_place(&p.ws));
        sessions.pin_workspace_cwd(p.ws.join("sub"));

        let out = sandbox_command(&p.ctx, "worktree");
        assert!(
            matches!(&out, SlashOutcome::Applied(t) if t.contains("换进了工作树")),
            "{out:?}"
        );
        let space = crate::session::cwd::session_space(&p.ctx);
        assert_eq!(space.home, p.ws);
        assert!(space.branch().unwrap().starts_with("dock/"));
        let store = cordis_base::config::dock_home()
            .canonicalize()
            .unwrap()
            .join(dock_sandbox::space::STORE_DIR);
        assert!(space.root.starts_with(&store), "{}", space.root.display());
        assert_eq!(
            crate::session::cwd::session_cwd(&p.ctx),
            space.root.join("sub")
        );
        assert!(space.root.join("sub/a.txt").is_file());
        assert!(
            matches!(sessions.events().last(), Some(cordis_base::types::LogEvent::SystemReminder(t)) if t.contains("工作树")),
            "模型要知道换了地方"
        );
        let write = |path: PathBuf| serde_json::json!({ "target_file": path }).to_string();
        assert_eq!(
            gate(&p.ctx, "write_file", &write(space.root.join("new.txt"))),
            Gate::SkipPrompt
        );
        assert_eq!(
            gate(&p.ctx, "write_file", &write(p.ws.join("new.txt"))),
            Gate::Default,
            "原项目不在工作树的可写根里"
        );
        assert!(
            matches!(sandbox_command(&p.ctx, "worktree"), SlashOutcome::Applied(t) if t.contains("已经在工作树里"))
        );

        let out = sandbox_command(&p.ctx, "inplace");
        assert!(
            matches!(&out, SlashOutcome::Applied(t) if t.contains("回到原项目")),
            "{out:?}"
        );
        assert_eq!(
            crate::session::cwd::session_space(&p.ctx),
            dock_sandbox::space::Space::in_place(&p.ws)
        );
        assert_eq!(crate::session::cwd::session_cwd(&p.ctx), p.ws.join("sub"));
        assert!(space.root.is_dir(), "工作树留着");
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args.to_string(),
        }
    }

    /// 跑一次调用；要是弹了权限框，按 `answer` 答掉并把弹框摘要交回来。
    async fn run(
        page: &Page,
        call: ToolCall,
        answer: PermissionOptionKind,
    ) -> (ToolResult, Option<String>) {
        let tools = page.tools.clone();
        let task = tokio::spawn(async move { tools.execute(call).await });
        let perms = page.ctx.get::<Permissions>(PERMISSIONS).unwrap();
        let mut asked = None;
        for _ in 0..2000 {
            if task.is_finished() {
                break;
            }
            if let Some(front) = perms.front() {
                asked = Some(front.summary.clone());
                perms.resolve(answer);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        (task.await.unwrap(), asked)
    }

    fn backend_ready() -> bool {
        match dock_sandbox::availability() {
            Availability::Ready { .. } => true,
            Availability::Unavailable { reason } => {
                // CI 上设了 DOCK_SANDBOX_REQUIRE：后端不可用就是失败，不悄悄跳过。
                assert!(
                    std::env::var_os("DOCK_SANDBOX_REQUIRE").is_none(),
                    "DOCK_SANDBOX_REQUIRE 设了，但沙箱后端不可用：{reason}"
                );
                eprintln!("SKIP: 沙箱后端不可用：{reason}");
                false
            }
        }
    }

    /// 沙箱开着、询问档：bash 不弹框直接在沙箱里跑；写工作区外被拦，结果里有提示；
    /// 模型要出沙箱（`"sandbox": false`）才问用户，弹框写明不进沙箱。
    #[tokio::test]
    async fn sandboxed_bash_skips_the_prompt_and_escaping_asks() {
        if !backend_ready() {
            return;
        }
        let p = page();
        let (out, asked) = run(
            &p,
            call(
                "bash",
                serde_json::json!({"command": "echo hi > inside.txt && cat inside.txt"}),
            ),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert_eq!(asked, None, "沙箱里的命令不该弹框");
        assert!(out.content.contains("hi"), "{}", out.content);
        assert!(p.ws.join("inside.txt").exists());

        let target = p.outside.join("f");
        let script = format!("echo x > '{}'", target.display());
        let (out, asked) = run(
            &p,
            call("bash", serde_json::json!({ "command": script })),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert_eq!(asked, None);
        assert!(!target.exists(), "工作区外写进去了");
        assert!(
            out.content.contains("[沙箱]"),
            "该提示可能被沙箱拦了：{}",
            out.content
        );

        let (_, asked) = run(
            &p,
            call(
                "bash",
                serde_json::json!({ "command": script, "sandbox": false }),
            ),
            PermissionOptionKind::AllowOnce,
        )
        .await;
        let asked = asked.expect("出沙箱要问用户");
        assert!(asked.contains("不进沙箱"), "{asked}");
        assert!(target.exists(), "批准后该在沙箱外跑");
    }

    /// 写文件工具：沙箱可写范围内不弹，外面照常弹。
    #[tokio::test]
    async fn write_tools_skip_the_prompt_only_inside_the_sandbox_boundary() {
        let p = page();
        let inside = p.ws.join("a.txt");
        let (out, asked) = run(
            &p,
            call(
                "write_file",
                serde_json::json!({"target_file": inside, "contents": "x"}),
            ),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert_eq!(asked, None, "{}", out.content);
        assert!(inside.exists());

        let protected = p.ws.join(".dock/config.toml");
        let (_, asked) = run(
            &p,
            call(
                "write_file",
                serde_json::json!({"target_file": protected, "contents": "x"}),
            ),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert!(asked.is_some(), ".dock 在沙箱里只读，写它要问");
        assert!(!protected.exists());

        let outside = p.outside.join("b.txt");
        let (_, asked) = run(
            &p,
            call(
                "write_file",
                serde_json::json!({"target_file": outside, "contents": "x"}),
            ),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert!(asked.is_some(), "工作区外要问");
        assert!(!outside.exists());
    }

    /// 读拒读名单里的路径（`~/.ssh`）：本来不过门的读工具也要问。用一个不存在的文件名，
    /// 测试不会真去读谁的密钥。
    #[tokio::test]
    async fn reading_a_secret_path_asks_first() {
        let p = page();
        let (out, asked) = run(
            &p,
            call(
                "read_file",
                serde_json::json!({"target_file": "~/.ssh/dock-sandbox-test-no-such-key"}),
            ),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert!(asked.is_some(), "读 ~/.ssh 要问");
        assert!(out.is_error);
    }

    /// 计划模式下沙箱开着：命令不问、在只读沙箱里跑——能读，写工作区被拦。
    #[tokio::test]
    async fn plan_mode_runs_commands_in_the_read_only_sandbox() {
        if !backend_ready() {
            return;
        }
        let p = page();
        let plan = PlanMode::new(p.ctx.clone());
        plan.enter_active();
        std::mem::forget(p.ctx.provide(PLAN_MODE, plan).unwrap());
        std::fs::write(p.ws.join("seen.txt"), "visible").unwrap();
        let (out, asked) = run(
            &p,
            call(
                "bash",
                serde_json::json!({"command": "cat seen.txt; echo x > new.txt"}),
            ),
            PermissionOptionKind::RejectOnce,
        )
        .await;
        assert_eq!(asked, None, "{}", out.content);
        assert!(out.content.contains("visible"), "{}", out.content);
        assert!(!p.ws.join("new.txt").exists(), "只读沙箱里写进去了");
    }

    /// 插件按用户级 `[sandbox] enabled` 给这一页播种；`/sandbox` 关、切、看，认不得的参数
    /// 不动开关。
    #[tokio::test]
    async fn the_plugin_seeds_from_config_and_slash_toggles_the_page() {
        use crate::host::slash::{slash, Slash};

        let _env = cordis_base::test_env::scoped().home();
        std::fs::write(
            cordis_base::config::dock_home().join("config.toml"),
            "[sandbox]\nenabled = true\n",
        )
        .unwrap();
        let root = Context::new();
        std::mem::forget(root.provide(SETTINGS, AppSettings::new("x")).unwrap());
        for p in [slash(), sandbox()] {
            root.plugin(p, ()).unwrap().wait().await.unwrap();
        }
        assert!(enabled(&root), "配置里开着，第 1 页该播种成开");

        let slash = root.get::<Slash>(SLASH).unwrap();
        let notice = |out: Option<SlashOutcome>| match out {
            Some(SlashOutcome::Notice { body, .. }) => body,
            other => panic!("该回 notice：{other:?}"),
        };
        let body = notice(slash.run(&root, "sandbox", "off").await);
        assert!(!enabled(&root));
        assert!(body.contains("这一页的沙箱：关"), "{body}");

        notice(slash.run(&root, "sandbox", "").await);
        assert!(enabled(&root), "不带参数是切换");

        let body = notice(slash.run(&root, "sandbox", "status").await);
        assert!(enabled(&root), "status 不改开关");
        assert!(body.contains("这一页的沙箱：开"), "{body}");

        let body = notice(slash.run(&root, "sandbox", "maybe").await);
        assert!(enabled(&root));
        assert!(body.contains("不认识"), "{body}");
    }

    #[test]
    fn gate_is_silent_while_the_sandbox_is_off() {
        let ctx = Context::new();
        let settings = AppSettings::new("x");
        let _s = ctx.provide(SETTINGS, settings).unwrap();
        assert_eq!(gate(&ctx, "bash", r#"{"command":"ls"}"#), Gate::Default);
        assert_eq!(
            gate(&ctx, "read_file", r#"{"target_file":"~/.ssh/id_rsa"}"#),
            Gate::Default
        );
    }
}

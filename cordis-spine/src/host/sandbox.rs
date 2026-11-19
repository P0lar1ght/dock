//! `sandbox` 插件：这一页的 `bash` / `monitor` 命令在系统沙箱里跑（[`dock_sandbox`]），
//! 沙箱里的命令不再弹权限框；`write_file` / `search_replace` 写在沙箱可写范围内也不弹。
//!
//! - 开关按页（[`AppSettings::sandbox`]）：挂载时按 `$DOCK_HOME/config.toml` 的
//!   `[sandbox] enabled` 给第 1 页播种，新开的页抄开它的那一页；`/sandbox` 切当前页。
//! - 策略每次现算（[`policy`]）：改了 `[sandbox]` 不用重启。
//! - 后端不可用（内核没有 Landlock 之类）时一切照旧：命令照常过权限门，不会不加防护地跑。
//!
//! 不变式：关卡判「沙箱里跑、不用问」（[`Gate::Sandboxed`]）的调用，执行时**一定**在沙箱里跑。
//! 这个决定由注册表经任务局部变量交给工具体（[`with_access`] / [`granted_access`]），不让
//! 工具体自己再判一次——判两次中间要是有人切了开关，就成了「没问、也没进沙箱」。包不上就
//! 报错不跑，绝不退回沙箱外。

use std::future::Future;
use std::path::{Path, PathBuf};

use cordis::{plugin, Context, Inject, Plugin};
use dock_sandbox::{Access, Availability, Host, Policy, Settings};
use serde_json::Value;

use crate::host::settings::AppSettings;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{SETTINGS, SLASH};

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
        network: cfg.network,
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

/// 这一页此刻的策略（工作区 = 这一页的会话 cwd）。
pub fn policy(page: &Context, access: Access) -> Policy {
    let workspace = crate::session::cwd::session_cwd(page);
    Policy::build(access, &workspace, &settings(), &host())
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
    if !enabled(exec) {
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

/// 给人看的现状。
fn describe(page: &Context) -> String {
    let on = enabled(page);
    let mut out = vec![format!("这一页的沙箱：{}", if on { "开" } else { "关" })];
    match dock_sandbox::availability() {
        Availability::Unavailable { reason } => {
            out.push(format!("本机不可用：{reason}"));
            out.push("开着也不起作用：命令照常走权限确认。".into());
        }
        Availability::Ready { backend, limits } => {
            let p = policy(page, Access::WorkspaceWrite);
            out.push(format!("后端：{}", backend.name()));
            out.push(format!("工作区：{}", p.workspace().display()));
            out.push(format!("网络：{}", if p.network() { "开" } else { "关" }));
            out.push(
                "可写：工作区、临时目录、$DOCK_HOME/scratch，以及依赖缓存和 [sandbox] writable_roots"
                    .into(),
            );
            out.push(
                "不可读：~/.ssh、~/.aws、钥匙串、Dock 的密钥等（[sandbox] deny_read 可加）".into(),
            );
            out.push("只读：.git/hooks、.git/config、.dock".into());
            for limit in limits {
                out.push(format!("限制：{}", limit.describe()));
            }
            if on {
                out.push(
                    "沙箱里的命令不再弹权限框；要出沙箱的（\"sandbox\": false）照常问你。".into(),
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
    match args.trim() {
        "" => settings.set_sandbox(!settings.sandbox()),
        "on" | "开" => settings.set_sandbox(true),
        "off" | "关" => settings.set_sandbox(false),
        "status" | "状态" => {}
        other => {
            return SlashOutcome::notice(
                "沙箱",
                format!("不认识「{other}」：用 /sandbox [on|off|status]，不带参数是切换。"),
            )
        }
    }
    SlashOutcome::notice("沙箱", describe(page))
}

/// `sandbox` 插件：播种第 1 页的开关、挂 `/sandbox`。
pub fn sandbox() -> Plugin {
    plugin("sandbox", Inject::from([SLASH]), |ctx, _: &()| {
        if let Some(settings) = ctx.get::<AppSettings>(SETTINGS) {
            settings.set_sandbox(cordis_base::config::load_sandbox_config().enabled);
        }
        register_commands(
            ctx,
            vec![SlashCommand::host(
                "sandbox",
                "开关这一页的命令沙箱（/sandbox on|off|status）",
                slash_handler(sandbox_command),
            )
            .takes_args(true)],
        )?;
        Ok(None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 一页：会话 cwd 钉在临时目录里的工作区，询问档，沙箱开着。
    struct Page {
        _dir: tempfile::TempDir,
        ctx: Context,
        ws: PathBuf,
        outside: PathBuf,
        tools: Tools,
    }

    fn page() -> Page {
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
            _dir: dir,
            ctx,
            ws,
            outside,
            tools,
        }
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

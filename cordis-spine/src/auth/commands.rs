//! `/auth` 命令组：订阅通道的登录、登出、状态、账号切换。
//!
//! ```text
//! /auth                              = /auth login
//! /auth login [new|identity|<账号>] [provider]
//!                                    登录：默认重新登录活动账号（登出了也算，复用原注册）；
//!                                    new = 再加一个账号；identity = 只要身份，不要套餐额度
//! /auth logout [账号|provider]       撤销 refresh token、清本地 token（注册与 host_id 留着）
//! /auth cancel [provider]            中断正在等授权的登录
//! /auth status                       各 provider 的活动账号、套餐授权、token 剩余时间
//! /auth list                         列出已保存的账号，标出活动账号与已登出的
//! /auth use <账号>                   切活动账号
//! /auth models [provider]            重拉可用模型并入模型目录（/model <provider>/<slug>）
//! /auth usage [provider]             订阅额度还剩多少（本周期已用百分比、重置时间）
//! /auth import <文件> [provider]     导入从别处传来的凭据（VPS 场景），用本机 host_id
//! ```
//!
//! 命令体只经 `"auth"` 服务与 provider trait 说话，不认识哪一家的凭据文件——
//! 第二家 provider 挂进来就能用。

use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};

use crate::auth::manager::{AuthManager, LoginState};
use crate::auth::BrowserLoginUi;
use crate::host::slash::{register_commands, Slash, SlashCommand, SlashOutcome};
use crate::names::{AUTH, SLASH};
use cordis_base::auth::{LoginRequest, LoginTarget, LogoutOutcome, ScopeMode};

const SUBCOMMANDS: &str = "login / logout / cancel / status / list / use / models / usage / import";

pub fn auth_commands() -> Plugin {
    plugin(
        "command-auth",
        Inject::from([AUTH, SLASH]),
        |ctx, _: &()| {
            register_commands(
                ctx,
                vec![SlashCommand::host(
                    "auth",
                    "订阅通道：登录 / 登出 / 状态 / 账号 / 模型（直接回车 = 登录）",
                    // 异步命令体：`slash_handler` 只收同步函数，这里自己包一层。
                    Arc::new(|ctx, args| Box::pin(run(ctx, args))),
                )
                .takes_args(true)],
            )?;
            Ok(None)
        },
    )
}

async fn run(page: Context, args: String) -> SlashOutcome {
    let Some(manager) = page.get::<AuthManager>(AUTH) else {
        return SlashOutcome::notice("鉴权", "鉴权服务未挂载。");
    };
    let mut parts = args.split_whitespace();
    // 不带参数 = **直接开始登录**（命令说明里就是这么写的）。想看状态用 `/auth status`。
    let sub = parts.next().unwrap_or("login").to_string();
    let rest: Vec<&str> = parts.collect();

    match sub.as_str() {
        "login" => login(&manager, &page, &rest),
        "logout" => logout(&manager, rest.first().copied()).await,
        "cancel" => cancel(&manager, rest.first().copied()),
        "status" => status(&manager),
        "list" => list(&manager),
        "use" => use_account(&manager, rest.first().copied()),
        "models" => models(&manager, rest.first().copied()).await,
        "usage" => usage(&manager, rest.first().copied()).await,
        "import" => import(&manager, &rest),
        other => SlashOutcome::Applied(format!("未知子命令 {other}。可用：{SUBCOMMANDS}")),
    }
}

/// 没写 provider 时用哪一家：只装了一家就是它，否则 chatgpt。
fn default_provider(manager: &AuthManager) -> String {
    let ids = manager.provider_ids();
    match ids.as_slice() {
        [only] => only.clone(),
        _ => crate::auth::chatgpt::PROVIDER_ID.to_string(),
    }
}

/// `/auth login [new|identity|<账号>] [provider]`。
///
/// **在后台跑**：授权要人在浏览器里点，可能一两分钟。命令不能一直占着——
/// 占着的话终端看着像卡住了，用户连 `/auth status` 都打不了。结果经提示弹出来。
fn login(manager: &Arc<AuthManager>, page: &Context, args: &[&str]) -> SlashOutcome {
    let ids = manager.provider_ids();
    let mut req = LoginRequest::default();
    let mut provider: Option<String> = None;
    for arg in args {
        match *arg {
            "identity" | "id" => req.scope = ScopeMode::IdentityOnly,
            "new" | "add" => req.target = LoginTarget::New,
            other if ids.iter().any(|id| id == other) => provider = Some(other.to_string()),
            account => {
                // 指名一个账号：顺便认出它是哪一家的。本机没有就当场说，不先开授权页。
                let Some((owner, _)) = manager.find_account(account) else {
                    return SlashOutcome::Applied(format!(
                        "本机没有这个账号：{account}。/auth list 看已保存的账号，/auth login new 加一个新的。"
                    ));
                };
                provider.get_or_insert(owner);
                req.target = LoginTarget::Account(account.to_string());
            }
        }
    }
    let provider = provider.unwrap_or_else(|| default_provider(manager));
    let Some(slash) = page.get::<Slash>(SLASH) else {
        return SlashOutcome::Applied("斜杠服务没有挂载".into());
    };
    let ui = Arc::new(BrowserLoginUi::with_notice(slash));
    let target = match &req.target {
        LoginTarget::Active => "活动账号".to_string(),
        LoginTarget::Account(id) => format!("账号 {id}"),
        LoginTarget::New => "新账号".to_string(),
    };
    let scope = req.scope;
    match manager.start_login(&provider, req, ui) {
        Ok(()) => SlashOutcome::Applied(format!(
            "正在打开 {provider} 的授权页（{target}，{scope}），在浏览器里完成授权就行（10 分钟内有效）。\n\
             完成后这里会提示；/auth status 看结果，/auth cancel 中断。"
        )),
        Err(e) => SlashOutcome::Applied(e.message()),
    }
}

/// `/auth logout [账号|provider]`：没给就是默认 provider 的活动账号；给的是 provider
/// id（`grok`）就是那一家的活动账号。
async fn logout(manager: &AuthManager, account: Option<&str>) -> SlashOutcome {
    let (provider, account) = match account {
        Some(id) if manager.provider_ids().iter().any(|p| p == id) => (id.to_string(), None),
        Some(id) => match manager.find_account(id) {
            Some((owner, _)) => (owner, Some(id)),
            None => return SlashOutcome::Applied(format!("本机没有这个账号：{id}")),
        },
        None => (default_provider(manager), None),
    };
    let name = manager
        .provider(&provider)
        .map(|p| p.info().display_name)
        .unwrap_or("这一家");
    match manager.logout(&provider, account, true).await {
        Ok((acct, LogoutOutcome::Revoked)) => SlashOutcome::Applied(format!(
            "已登出并撤销 {}（{provider}）。账号还在 /auth list 里：/auth login {} 重新登录。",
            acct.client_id, acct.client_id
        )),
        Ok((acct, LogoutOutcome::LocalOnly { reason })) => SlashOutcome::Applied(format!(
            "已清掉 {} 的本地 token，但远端撤销没确认（{reason}）。可以到 {name} 的账号设置里手动断开 Dock。",
            acct.client_id
        )),
        Err(e) => SlashOutcome::Applied(e.message()),
    }
}

/// `/auth cancel [provider]`。
fn cancel(manager: &AuthManager, provider: Option<&str>) -> SlashOutcome {
    let provider = provider
        .map(str::to_string)
        .unwrap_or_else(|| default_provider(manager));
    if manager.cancel_login(&provider) {
        SlashOutcome::Applied(format!("已中断 {provider} 的登录。"))
    } else {
        SlashOutcome::Applied(format!("{provider} 没有正在等授权的登录。"))
    }
}

/// `/auth status`。
fn status(manager: &AuthManager) -> SlashOutcome {
    let ids = manager.provider_ids();
    if ids.is_empty() {
        return SlashOutcome::notice("鉴权", "没有装任何鉴权插件。");
    }
    let now = crate::auth::credentials::now_unix();
    let mut body = String::new();
    for id in ids {
        let accounts = manager.accounts(&id);
        match accounts.iter().find(|a| a.active) {
            Some(a) => {
                let who = a
                    .email
                    .clone()
                    .unwrap_or_else(|| a.account.client_id.clone());
                let line = if let Some(problem) = &a.problem {
                    format!("{id}：{who}（凭据读不了：{problem}）")
                } else if !a.signed_in {
                    format!("{id}：{who}（已登出，/auth login 重新登录）")
                } else {
                    let plan = if a.plan_usage {
                        "套餐额度已开"
                    } else {
                        "套餐额度未开"
                    };
                    let left = a.expires_at.saturating_sub(now);
                    format!("{id}：{who}（{plan}，access token 还剩 {left}s）")
                };
                body.push_str(&line);
            }
            None => body.push_str(&format!("{id}：还没登录")),
        }
        match manager.login_state(&id) {
            Some(LoginState::Pending { .. }) => body.push_str(" · 正在等授权"),
            Some(LoginState::Failed { message }) => {
                body.push_str(&format!(" · 上次登录失败：{message}"))
            }
            _ => {}
        }
        body.push('\n');
    }
    let mut text = body.trim_end().to_string();
    text.push_str(&format!("\n\n其余子命令：{SUBCOMMANDS}"));
    SlashOutcome::notice("鉴权", text)
}

/// `/auth list`。
fn list(manager: &AuthManager) -> SlashOutcome {
    let mut lines = Vec::new();
    for id in manager.provider_ids() {
        for a in manager.accounts(&id) {
            let state = if let Some(problem) = &a.problem {
                format!("读不了：{problem}")
            } else if !a.signed_in {
                "已登出".to_string()
            } else if a.plan_usage {
                "套餐额度已开".to_string()
            } else {
                "套餐额度未开".to_string()
            };
            let mark = if a.active { " ·活动" } else { "" };
            lines.push(format!(
                "{id} · {}（{}，{state}）{mark}",
                a.account.client_id,
                a.email.clone().unwrap_or_else(|| "-".into()),
            ));
        }
    }
    if lines.is_empty() {
        return SlashOutcome::notice("鉴权", "还没有保存任何账号。");
    }
    SlashOutcome::notice("鉴权", lines.join("\n"))
}

/// `/auth use <账号>`。
fn use_account(manager: &AuthManager, account: Option<&str>) -> SlashOutcome {
    let Some(account) = account else {
        return SlashOutcome::Applied("用法：/auth use <账号>（/auth list 里那一列 id）".into());
    };
    let Some((provider, _)) = manager.find_account(account) else {
        return SlashOutcome::Applied(format!("本机没有这个账号：{account}"));
    };
    match manager.set_active(&provider, account) {
        Ok(key) => SlashOutcome::Applied(format!("活动账号：{}（{provider}）", key.client_id)),
        Err(e) => SlashOutcome::Applied(e.message()),
    }
}

/// `/auth models [provider]`：重拉活动账号能用的模型，**并入模型目录**
/// （`/model <provider>/<slug>` 就能切），列出来。
async fn models(manager: &AuthManager, provider: Option<&str>) -> SlashOutcome {
    let id = provider
        .map(str::to_string)
        .unwrap_or_else(|| default_provider(manager));
    match manager.refresh_models(&id).await {
        Ok(list) => {
            if list.is_empty() {
                return SlashOutcome::notice("模型", "这个账号没有可用的模型。");
            }
            let mut body = list
                .iter()
                .map(|m| {
                    let mut line = format!(
                        "{}  {}",
                        cordis_base::config::auth_models::model_id(&id, &m.slug),
                        m.display_name.as_deref().unwrap_or(&m.slug)
                    );
                    if let Some(window) = m.context_window {
                        line.push_str(&format!(" · {}K", window / 1000));
                    }
                    if let Some(efforts) = m.reasoning_efforts.as_deref().filter(|e| !e.is_empty())
                    {
                        line.push_str(&format!(" · 推理 {}", efforts.join("/")));
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n");
            body.push_str("\n\n已并入模型目录：/model <上面那列 id> 切过去。");
            SlashOutcome::notice("模型", body)
        }
        Err(e) => SlashOutcome::Applied(e.message()),
    }
}

/// `/auth usage [provider]`：订阅额度还剩多少。没写 provider 就列每一家登着的。
async fn usage(manager: &AuthManager, provider: Option<&str>) -> SlashOutcome {
    let ids: Vec<String> = match provider {
        Some(p) => vec![p.to_string()],
        None => manager
            .provider_ids()
            .into_iter()
            .filter(|id| manager.active_account(id).is_some())
            .collect(),
    };
    if ids.is_empty() {
        return SlashOutcome::Applied("还没有登录任何订阅通道：/auth login".into());
    }
    let now = crate::auth::credentials::now_unix();
    let mut lines = Vec::new();
    for id in ids {
        let name = manager
            .provider(&id)
            .map(|p| p.info().display_name)
            .unwrap_or("这一家");
        lines.push(match manager.usage(&id).await {
            Ok(Some(u)) => usage_line(name, &u, now),
            Ok(None) => format!("{name}：这一家查不了额度，到它的用量页看。"),
            Err(e) => format!("{name}：{}", e.message()),
        });
    }
    SlashOutcome::notice("订阅额度", lines.join("\n"))
}

/// 一家的额度，一行：`Grok（SuperGrok）：本周已用 27%，还剩 73% · 2 天 14 小时后重置`。
fn usage_line(name: &str, u: &cordis_base::auth::PlanUsage, now: u64) -> String {
    let who = match &u.plan {
        Some(plan) => format!("{name}（{plan}）"),
        None => name.to_string(),
    };
    let period = match u.period.as_ref().and_then(|p| p.kind.as_deref()) {
        Some("weekly") => "本周",
        Some("monthly") => "本月",
        _ => "本周期",
    };
    let mut line = match u.used_percent {
        Some(used) => format!(
            "{who}：{period}已用 {}%，还剩 {}%",
            percent(used),
            percent(100.0 - used)
        ),
        None => format!("{who}：上游没给已用多少"),
    };
    if let Some(end) = u.period.as_ref().and_then(|p| p.end) {
        if end > now {
            line.push_str(&format!(" · {}后重置", span(end - now)));
        }
    }
    if let Some(cents) = u.prepaid_balance_cents.filter(|c| *c > 0) {
        line.push_str(&format!(" · 预付余额 ${:.2}", cents as f64 / 100.0));
    }
    line
}

/// 百分比：整数就不带小数，否则一位。
fn percent(p: f64) -> String {
    let p = p.clamp(0.0, 100.0);
    if (p - p.round()).abs() < 0.05 {
        format!("{}", p.round() as i64)
    } else {
        format!("{p:.1}")
    }
}

/// 一段时长：`2 天 14 小时` / `5 小时 3 分` / `12 分`。
fn span(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d} 天 {h} 小时")
    } else if h > 0 {
        format!("{h} 小时 {m} 分")
    } else {
        format!("{} 分", m.max(1))
    }
}

/// `/auth import <文件> [provider]`：把别处传来的凭据装进来（VPS 场景）。
///
/// 官方那套是「本地登录 → 经 SSH 之类的安全通道传凭据 → 远端用自己的 host id
/// 接管刷新」。两边都刷会撞上 refresh token 轮换，所以传过去之后本地那份别再用。
fn import(manager: &AuthManager, args: &[&str]) -> SlashOutcome {
    let Some(file) = args.first() else {
        return SlashOutcome::Applied("用法：/auth import <凭据文件> [provider]".into());
    };
    let provider = args
        .get(1)
        .map(|p| p.to_string())
        .unwrap_or_else(|| default_provider(manager));
    let raw = match std::fs::read_to_string(file) {
        Ok(raw) => raw,
        Err(e) => return SlashOutcome::Applied(format!("读不了 {file}：{e}")),
    };
    match manager.import(&provider, &raw) {
        Ok(key) => SlashOutcome::Applied(format!(
            "已导入 {}（{provider}）。刷新由这台机器接管（用本机 host_id）；原来那台机器上的那份别再用了。",
            key.client_id
        )),
        Err(e) => SlashOutcome::Applied(e.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::auth::{PlanUsage, UsagePeriod};

    #[test]
    fn a_usage_line_says_what_is_left_and_when_it_resets() {
        let now = 1_000_000;
        let u = PlanUsage {
            plan: Some("SuperGrok".into()),
            used_percent: Some(27.0),
            period: Some(UsagePeriod {
                kind: Some("weekly".into()),
                start: None,
                end: Some(now + 2 * 86_400 + 14 * 3600 + 59),
            }),
            prepaid_balance_cents: Some(0),
            fetched_at: now,
        };
        assert_eq!(
            usage_line("Grok", &u, now),
            "Grok（SuperGrok）：本周已用 27%，还剩 73% · 2 天 14 小时后重置"
        );
        let u = PlanUsage {
            plan: None,
            used_percent: Some(12.34),
            period: None,
            prepaid_balance_cents: Some(1250),
            fetched_at: now,
        };
        assert_eq!(
            usage_line("Grok", &u, now),
            "Grok：本周期已用 12.3%，还剩 87.7% · 预付余额 $12.50"
        );
        let u = PlanUsage {
            used_percent: None,
            ..PlanUsage::default()
        };
        assert_eq!(usage_line("Grok", &u, now), "Grok：上游没给已用多少");
        assert_eq!(span(59), "1 分");
        assert_eq!(span(3 * 3600 + 120), "3 小时 2 分");
    }
}

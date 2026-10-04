//! `vcs/pr/*`：项目的 GitHub Pull Request（只读），经本机 `gh` CLI 查。
//!
//! - `vcs/pr/list { threadId | cwd }` → 这个项目开着的 PR，标出「我的」「请我 review」。
//! - `vcs/pr/get { threadId | cwd, number }` → 一个 PR 的详情（检查、review、改动量）。
//!
//! 项目只能是 Dock 认识的：开着的页或会话列表里的某个 cwd，和 `fs/*` 一样不出会话工作区。
//! **gh 用不了不是错误**：回 `{ available: false, reason, hint }`，客户端画空状态——
//! `gh_missing`（没装）/ `gh_unauthenticated`（没登录）/ `not_git`（不是 git 仓库）/
//! `not_github`（没有 GitHub 远端）/ `gh_failed`（gh 自己出错，`hint` 带它的原话）。
//!
//! 找 gh：`DOCK_GH`（给了就只认它）→ `PATH` → Homebrew / `~/.local/bin`。从访达启动的
//! 桌面端拿到的 `PATH` 通常不含 Homebrew，所以后两处要自己看。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use cordis_spine::session_cwd;

use crate::handle::GatewayHandle;
use crate::handlers::thread::roster_entries;
use crate::protocol::RpcError;
use crate::threads;

/// 指定 gh 路径的环境变量。
pub const GH_ENV: &str = "DOCK_GH";
/// 一次 gh 调用最多等这么久（它要走网络）。
const GH_TIMEOUT: Duration = Duration::from_secs(20);
/// 列表最多拿这么多条。
const LIST_LIMIT: &str = "50";

const LIST_FIELDS: &str = "number,title,url,author,isDraft,headRefName,baseRefName,updatedAt,\
reviewDecision,statusCheckRollup,mergeable,reviewRequests";
const VIEW_FIELDS: &str = "number,title,body,url,author,state,isDraft,headRefName,baseRefName,\
mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,additions,deletions,changedFiles,\
createdAt,updatedAt,reviews,comments";

pub async fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cwd = project_cwd(gateway, &params)?;
    run_blocking(move || list_in(&cwd)).await
}

pub async fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cwd = project_cwd(gateway, &params)?;
    let number = params
        .get("number")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(|| RpcError::invalid_params("number 是 PR 编号（正整数）"))?;
    run_blocking(move || get_in(&cwd, number)).await
}

async fn run_blocking(f: impl FnOnce() -> Value + Send + 'static) -> Result<Value, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::app("internal", format!("gh 查询没跑完：{e}")))
}

fn list_in(cwd: &Path) -> Value {
    let gh = match ready(cwd) {
        Ok(gh) => gh,
        Err(unavailable) => return unavailable,
    };
    let repo = match gh_json(&gh, cwd, &["repo", "view", "--json", "nameWithOwner"]) {
        Ok(v) => v["nameWithOwner"].as_str().unwrap_or_default().to_string(),
        Err(e) => return e.into_json(),
    };
    let viewer = match gh_json(&gh, cwd, &["api", "user"]) {
        Ok(v) => v["login"].as_str().unwrap_or_default().to_string(),
        Err(e) => return e.into_json(),
    };
    let raw = match gh_json(
        &gh,
        cwd,
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            LIST_LIMIT,
            "--json",
            LIST_FIELDS,
        ],
    ) {
        Ok(v) => v,
        Err(e) => return e.into_json(),
    };
    let prs: Vec<Value> = raw
        .as_array()
        .into_iter()
        .flatten()
        .map(|pr| list_row(pr, &viewer))
        .collect();
    json!({ "available": true, "repo": repo, "viewer": viewer, "prs": prs })
}

fn get_in(cwd: &Path, number: u64) -> Value {
    let gh = match ready(cwd) {
        Ok(gh) => gh,
        Err(unavailable) => return unavailable,
    };
    let n = number.to_string();
    match gh_json(&gh, cwd, &["pr", "view", &n, "--json", VIEW_FIELDS]) {
        Ok(pr) => json!({ "available": true, "pr": detail(&pr) }),
        Err(e) => e.into_json(),
    }
}

/// gh 在不在、目录是不是 git 仓库。都过了回 gh 的路径。
fn ready(cwd: &Path) -> Result<PathBuf, Value> {
    let Some(gh) = discover_gh() else {
        return Err(Unavailable::new(
            "gh_missing",
            "没找到 gh（GitHub CLI）。安装：brew install gh，然后在终端运行 gh auth login。",
        )
        .into_json());
    };
    if !cwd.join(".git").exists() && !inside_git(cwd) {
        return Err(Unavailable::new("not_git", "这个项目不是 git 仓库。").into_json());
    }
    Ok(gh)
}

/// 目录在某个 git 工作树里吗（子目录、worktree 也算）。没有 git 就当不是。
fn inside_git(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// 找 gh：`DOCK_GH` → `PATH` → 常见安装位置。
pub fn discover_gh() -> Option<PathBuf> {
    discover_gh_with(
        std::env::var_os(GH_ENV),
        std::env::var_os("PATH"),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn discover_gh_with(
    explicit: Option<std::ffi::OsString>,
    path_var: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(raw) = explicit {
        let text = raw.to_string_lossy().trim().to_string();
        if !text.is_empty() {
            // 显式给了就只认它：指错了要看得见「没装」，不悄悄回落。
            let path = PathBuf::from(text);
            return is_exec(&path).then_some(path);
        }
    }
    let on_path = path_var
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("gh"));
    let fallbacks = [
        Some(PathBuf::from("/opt/homebrew/bin/gh")),
        Some(PathBuf::from("/usr/local/bin/gh")),
        home.map(|h| h.join(".local/bin/gh")),
    ];
    on_path
        .chain(fallbacks.into_iter().flatten())
        .find(|p| is_exec(p))
}

fn is_exec(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// gh 用不了的原因：作为结果回给客户端，不是 RPC 错误。
struct Unavailable {
    reason: &'static str,
    hint: String,
}

impl Unavailable {
    fn new(reason: &'static str, hint: impl Into<String>) -> Self {
        Self {
            reason,
            hint: hint.into(),
        }
    }

    fn into_json(self) -> Value {
        json!({ "available": false, "reason": self.reason, "hint": self.hint })
    }
}

/// 跑一次 gh，stdout 当 JSON 解析。失败按 stderr 分成几类原因。
fn gh_json(gh: &Path, cwd: &Path, args: &[&str]) -> Result<Value, Unavailable> {
    let (code, stdout, stderr) = run(gh, cwd, args)?;
    if code != Some(0) {
        return Err(classify_failure(code, &stderr));
    }
    serde_json::from_str(&stdout)
        .map_err(|e| Unavailable::new("gh_failed", format!("gh 输出看不懂：{e}")))
}

fn classify_failure(code: Option<i32>, stderr: &str) -> Unavailable {
    let lower = stderr.to_ascii_lowercase();
    // gh 约定退出码 4 = 需要登录。
    if code == Some(4) || lower.contains("gh auth login") || lower.contains("not logged in") {
        return Unavailable::new(
            "gh_unauthenticated",
            "gh 还没登录 GitHub：在终端运行 gh auth login。",
        );
    }
    if lower.contains("not a git repository") {
        return Unavailable::new("not_git", "这个项目不是 git 仓库。");
    }
    if lower.contains("none of the git remotes")
        || lower.contains("no git remotes")
        || lower.contains("unable to determine")
    {
        return Unavailable::new("not_github", "这个仓库没有指向 GitHub 的远端。");
    }
    let line = stderr.trim().lines().last().unwrap_or("").trim();
    let hint = if line.is_empty() {
        "gh 执行失败。".to_string()
    } else {
        format!("gh 执行失败：{line}")
    };
    Unavailable::new("gh_failed", hint)
}

/// 起子进程、读完两路输出、限时。返回（退出码，stdout，stderr）。
fn run(gh: &Path, cwd: &Path, args: &[&str]) -> Result<(Option<i32>, String, String), Unavailable> {
    let mut child = Command::new(gh)
        .args(args)
        .current_dir(cwd)
        // 不要交互、不要颜色和分页器。
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("GH_PAGER", "")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Unavailable::new("gh_failed", format!("gh 起不来：{e}")))?;
    // 两路各开一个线程读完：只等退出、不读管道，输出一多子进程就卡在写上。
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut out);
            }
            out
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + GH_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Unavailable::new(
                    "gh_failed",
                    format!("gh 超过 {} 秒没回（网络？）", GH_TIMEOUT.as_secs()),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => return Err(Unavailable::new("gh_failed", format!("等 gh 失败：{e}"))),
        }
    };
    Ok((
        status.code(),
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    ))
}

/// 列表一行：只留界面要的字段，检查汇总成一个状态。
fn list_row(pr: &Value, viewer: &str) -> Value {
    let author = login(&pr["author"]);
    let requested = pr["reviewRequests"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|r| !viewer.is_empty() && login(r) == viewer);
    json!({
        "number": pr["number"],
        "title": pr["title"],
        "url": pr["url"],
        "author": author,
        "isDraft": pr["isDraft"].as_bool().unwrap_or(false),
        "headRefName": pr["headRefName"],
        "baseRefName": pr["baseRefName"],
        "updatedAt": pr["updatedAt"],
        "reviewDecision": non_empty(&pr["reviewDecision"]),
        "mergeable": non_empty(&pr["mergeable"]),
        "checks": checks_summary(&pr["statusCheckRollup"]),
        "mine": !viewer.is_empty() && author == viewer,
        "reviewRequested": requested,
    })
}

fn detail(pr: &Value) -> Value {
    let checks: Vec<Value> = pr["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            json!({
                "name": c["name"].as_str().or(c["context"].as_str()).unwrap_or(""),
                "state": check_state(c),
                "url": c["detailsUrl"].as_str().or(c["targetUrl"].as_str()),
            })
        })
        .collect();
    // 每个人只留最近一次 review。
    let mut reviews: Vec<Value> = Vec::new();
    for r in pr["reviews"].as_array().into_iter().flatten() {
        let who = login(&r["author"]);
        reviews.retain(|x| x["author"] != json!(who));
        reviews.push(json!({
            "author": who,
            "state": r["state"],
            "submittedAt": r["submittedAt"],
        }));
    }
    json!({
        "number": pr["number"],
        "title": pr["title"],
        "body": pr["body"].as_str().unwrap_or(""),
        "url": pr["url"],
        "author": login(&pr["author"]),
        "state": pr["state"],
        "isDraft": pr["isDraft"].as_bool().unwrap_or(false),
        "headRefName": pr["headRefName"],
        "baseRefName": pr["baseRefName"],
        "mergeable": non_empty(&pr["mergeable"]),
        "mergeStateStatus": non_empty(&pr["mergeStateStatus"]),
        "reviewDecision": non_empty(&pr["reviewDecision"]),
        "additions": pr["additions"],
        "deletions": pr["deletions"],
        "changedFiles": pr["changedFiles"],
        "createdAt": pr["createdAt"],
        "updatedAt": pr["updatedAt"],
        "comments": pr["comments"].as_array().map_or(0, Vec::len),
        "reviews": reviews,
        "checksSummary": checks_summary(&pr["statusCheckRollup"]),
        "checks": checks,
    })
}

/// 一项检查的状态：`success` / `failure` / `pending` / `skipped`。
/// `CheckRun` 看 `status` + `conclusion`，`StatusContext` 看 `state`。
fn check_state(c: &Value) -> &'static str {
    let up = |v: &Value| v.as_str().unwrap_or("").to_ascii_uppercase();
    let conclusion = up(&c["conclusion"]);
    let status = up(&c["status"]);
    let state = up(&c["state"]);
    if !status.is_empty() && status != "COMPLETED" {
        return "pending";
    }
    match (conclusion.as_str(), state.as_str()) {
        ("SUCCESS", _) | (_, "SUCCESS") => "success",
        ("NEUTRAL" | "SKIPPED", _) => "skipped",
        ("", "PENDING" | "EXPECTED") => "pending",
        ("", "") => "pending",
        _ => "failure",
    }
}

/// 检查汇总：有失败就 `failure`，有在跑就 `pending`，全过 `success`，没有检查 `none`。
fn checks_summary(rollup: &Value) -> Value {
    let states: Vec<&str> = rollup
        .as_array()
        .into_iter()
        .flatten()
        .map(check_state)
        .collect();
    let count = |s: &str| states.iter().filter(|x| **x == s).count();
    let (failed, pending) = (count("failure"), count("pending"));
    let state = if states.is_empty() {
        "none"
    } else if failed > 0 {
        "failure"
    } else if pending > 0 {
        "pending"
    } else {
        "success"
    };
    json!({ "state": state, "total": states.len(), "failed": failed, "pending": pending })
}

fn login(v: &Value) -> String {
    v["login"].as_str().unwrap_or_default().to_string()
}

fn non_empty(v: &Value) -> Value {
    match v.as_str() {
        Some(s) if !s.is_empty() => json!(s),
        _ => Value::Null,
    }
}

/// 项目目录：`threadId` 取那个线程的 cwd；`cwd` 必须是 Dock 认识的某个项目
/// （开着的页或会话列表里的 cwd），不让客户端拿它去任意目录跑 gh。
fn project_cwd(gateway: &GatewayHandle, params: &Value) -> Result<PathBuf, RpcError> {
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(thread) = text("threadId") {
        if let Ok(page) = threads::resolve(gateway, thread) {
            return Ok(session_cwd(&page.ctx));
        }
        return roster_entries(gateway)
            .into_iter()
            .find(|e| e.id == thread)
            .map(|e| e.cwd)
            .ok_or_else(|| RpcError::app("not_found", format!("thread {thread} not found")));
    }
    let raw = text("cwd").ok_or_else(|| RpcError::invalid_params("threadId 或 cwd 必填"))?;
    let wanted = canonical(Path::new(raw));
    let known = threads::open_pages(gateway)
        .iter()
        .map(|p| session_cwd(&p.ctx))
        .chain(roster_entries(gateway).into_iter().map(|e| e.cwd))
        .any(|c| canonical(&c) == wanted);
    if !known {
        return Err(RpcError::app(
            "invalid_params",
            format!("{raw} 不是 Dock 认识的项目（没有会话在这个目录下）"),
        ));
    }
    Ok(wanted)
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_gh_path_is_the_only_one_tried() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert_eq!(
            discover_gh_with(Some(missing.into_os_string()), None, None),
            None,
            "给错了路径就是没装，不回落到 PATH"
        );
    }

    #[test]
    fn checks_roll_up_failure_over_pending_over_success() {
        let rollup = json!([
            { "__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS" },
            { "__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS", "conclusion": "" },
            { "__typename": "StatusContext", "context": "ci/x", "state": "FAILURE" },
            { "__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED" }
        ]);
        let s = checks_summary(&rollup);
        assert_eq!(s["state"], "failure");
        assert_eq!(s["total"], 4);
        assert_eq!(s["failed"], 1);
        assert_eq!(s["pending"], 1);
        assert_eq!(checks_summary(&json!([]))["state"], "none");
        assert_eq!(
            checks_summary(&json!([{ "status": "COMPLETED", "conclusion": "SUCCESS" }]))["state"],
            "success"
        );
    }

    #[test]
    fn rows_mark_mine_and_review_requested() {
        let pr = json!({
            "number": 7, "title": "t", "url": "u", "author": { "login": "me" },
            "isDraft": false, "reviewDecision": "", "mergeable": "MERGEABLE",
            "reviewRequests": [{ "login": "me" }], "statusCheckRollup": []
        });
        let row = list_row(&pr, "me");
        assert_eq!(row["mine"], true);
        assert_eq!(row["reviewRequested"], true);
        assert_eq!(row["reviewDecision"], Value::Null, "空串当没有");
        assert_eq!(list_row(&pr, "")["mine"], false, "不知道我是谁就都不算");
    }

    #[test]
    fn failures_are_classified_for_the_empty_state() {
        assert_eq!(classify_failure(Some(4), "").reason, "gh_unauthenticated");
        assert_eq!(
            classify_failure(
                Some(1),
                "To get started with GitHub CLI, please run:  gh auth login"
            )
            .reason,
            "gh_unauthenticated"
        );
        assert_eq!(
            classify_failure(Some(1), "none of the git remotes configured for this repository point to a known GitHub host").reason,
            "not_github"
        );
        let other = classify_failure(Some(1), "HTTP 502\nsomething broke");
        assert_eq!(other.reason, "gh_failed");
        assert!(other.hint.contains("something broke"), "{}", other.hint);
    }
}

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
//!
//! **慢的是建连接**（每次 gh 都新开一条 TLS，走代理时一次就要一两秒），不是传数据，所以：
//! - 列表只发**一次** `gh api graphql`（我是谁 + 仓库名 + PR，`{owner}/{repo}` 由 gh 在本地解析）。
//! - 列表按项目缓存 [`LIST_TTL`]：有效期内直接回（`cached: true`）。判断「变没变」本身就要
//!   一次往返（GraphQL 没有 304），省不掉，所以只能靠有效期；`force: true` 绕过。
//! - 详情按 PR 缓存，列表里这个 PR 的 `updatedAt` 和检查汇总都没变就直接回，不再问 GitHub
//!   （检查跑完不一定动 `updatedAt`，所以两样一起看）。
//! - 同一个 key 同时来的请求只跑一次 gh，后来的等它的结果。gh 用不了的结果不缓存。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
/// 列表缓存多久。
pub const LIST_TTL: Duration = Duration::from_secs(60);
/// 详情在列表里看不出变没变时（列表过期了）缓存多久。
const DETAIL_TTL: Duration = Duration::from_secs(60);

/// 一次往返拿齐列表要的东西。`{owner}` / `{repo}` 由 gh 按当前目录的仓库在本地替换。
const LIST_QUERY: &str = "query($owner:String!,$name:String!){viewer{login}\
repository(owner:$owner,name:$name){nameWithOwner \
pullRequests(states:OPEN,first:50,orderBy:{field:UPDATED_AT,direction:DESC}){nodes{\
number title url isDraft headRefName baseRefName updatedAt reviewDecision mergeable author{login} \
reviewRequests(first:20){nodes{requestedReviewer{... on User{login}}}} \
commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{__typename \
... on CheckRun{name status conclusion detailsUrl} ... on StatusContext{context state targetUrl}}}}}}}}}}}";
const VIEW_FIELDS: &str = "number,title,body,url,author,state,isDraft,headRefName,baseRefName,\
mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,additions,deletions,changedFiles,\
createdAt,updatedAt,reviews,comments";

pub async fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cwd = project_cwd(gateway, &params)?;
    let force = force(&params);
    let key = format!("list:{}", cwd.display());
    let _one = in_flight(&key).lock_owned().await;
    if !force {
        if let Some(hit) = cache().lock().unwrap().lists.get(&cwd) {
            if hit.at.elapsed() < LIST_TTL {
                return Ok(stamped(&hit.value, hit.fetched_ms, true));
            }
        }
    }
    let value = run_blocking({
        let cwd = cwd.clone();
        move || list_in(&cwd)
    })
    .await?;
    let fetched_ms = now_ms();
    if value["available"] == true {
        cache().lock().unwrap().lists.insert(
            cwd,
            Cached {
                at: Instant::now(),
                fetched_ms,
                value: value.clone(),
            },
        );
    }
    Ok(stamped(&value, fetched_ms, false))
}

pub async fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cwd = project_cwd(gateway, &params)?;
    let number = params
        .get("number")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(|| RpcError::invalid_params("number 是 PR 编号（正整数）"))?;
    let force = force(&params);
    let key = format!("pr:{}#{number}", cwd.display());
    let _one = in_flight(&key).lock_owned().await;
    if !force {
        let cache = cache().lock().unwrap();
        if let Some(hit) = cache.details.get(&(cwd.clone(), number)) {
            // 有比这份详情新的列表：只看列表里这个 PR 变没变（变了就算还在有效期内也重拉）。
            // 没有更新的列表：按有效期。
            let fresh = match cache.lists.get(&cwd).filter(|list| list.at >= hit.at) {
                Some(list) => signature(&list.value, number).as_ref() == Some(&hit.signature),
                None => hit.at.elapsed() < DETAIL_TTL,
            };
            if fresh {
                return Ok(stamped(&hit.value, hit.fetched_ms, true));
            }
        }
    }
    let value = run_blocking({
        let cwd = cwd.clone();
        move || get_in(&cwd, number)
    })
    .await?;
    let fetched_ms = now_ms();
    if value["available"] == true {
        let pr = &value["pr"];
        let signature = format!("{}|{}", pr["updatedAt"], pr["checksSummary"]);
        cache().lock().unwrap().details.insert(
            (cwd, number),
            CachedDetail {
                at: Instant::now(),
                fetched_ms,
                signature,
                value: value.clone(),
            },
        );
    }
    Ok(stamped(&value, fetched_ms, false))
}

fn force(params: &Value) -> bool {
    params
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 回给客户端时盖上「什么时候从 GitHub 拿的」「这次是不是缓存」。
fn stamped(value: &Value, fetched_ms: u64, cached: bool) -> Value {
    let mut out = value.clone();
    if let Some(obj) = out.as_object_mut() {
        if obj.get("available") == Some(&Value::Bool(true)) {
            obj.insert("fetchedAtMs".into(), json!(fetched_ms));
            obj.insert("cached".into(), json!(cached));
        }
    }
    out
}

/// 列表里一个 PR 的「变没变」：`updatedAt` + 检查汇总，和详情缓存时记的同一个格式。
fn signature(list: &Value, number: u64) -> Option<String> {
    let pr = list["prs"]
        .as_array()?
        .iter()
        .find(|p| p["number"].as_u64() == Some(number))?;
    Some(format!("{}|{}", pr["updatedAt"], pr["checks"]))
}

struct Cached {
    at: Instant,
    fetched_ms: u64,
    value: Value,
}

struct CachedDetail {
    at: Instant,
    fetched_ms: u64,
    signature: String,
    value: Value,
}

#[derive(Default)]
struct Cache {
    lists: HashMap<PathBuf, Cached>,
    details: HashMap<(PathBuf, u64), CachedDetail>,
}

/// 进程内一份：多个窗口 / 客户端共用。
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 同一个 key 一次只跑一次 gh：后来的拿到锁时缓存已经是新的了。
fn in_flight(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(key.to_string())
        .or_default()
        .clone()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    let query = format!("query={LIST_QUERY}");
    let raw = match gh_json(
        &gh,
        cwd,
        &[
            "api",
            "graphql",
            "-f",
            &query,
            "-F",
            "owner={owner}",
            "-F",
            "name={repo}",
        ],
    ) {
        Ok(v) => v,
        Err(e) => return e.into_json(),
    };
    let data = &raw["data"];
    let viewer = data["viewer"]["login"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let repo = &data["repository"];
    let prs: Vec<Value> = repo["pullRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| list_row(&from_graphql(node), &viewer))
        .collect();
    json!({
        "available": true,
        "repo": repo["nameWithOwner"].as_str().unwrap_or_default(),
        "viewer": viewer,
        "prs": prs,
    })
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

/// GraphQL 的 PR 节点 → 和 `gh pr list --json` 同形（`reviewRequests[].login`、
/// `statusCheckRollup[]` 是最后一个提交的检查），后面的整理两边共用。
fn from_graphql(node: &Value) -> Value {
    let mut pr = node.clone();
    let requests: Vec<Value> = node["reviewRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r["requestedReviewer"].clone())
        .collect();
    let checks =
        node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"].clone();
    if let Some(obj) = pr.as_object_mut() {
        obj.insert("reviewRequests".into(), Value::Array(requests));
        obj.insert(
            "statusCheckRollup".into(),
            if checks.is_array() { checks } else { json!([]) },
        );
        obj.remove("commits");
    }
    pr
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

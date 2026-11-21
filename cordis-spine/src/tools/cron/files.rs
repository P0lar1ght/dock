//! 「文件变化」触发：给本进程持有的文件变化任务挂 notify 监听，攒下改动，安静够久
//! （防抖）再交出来。驱动拿到后 [`super::Cron::trigger`]。
//!
//! 路径相对任务的工作目录；`~/` 开头算家目录；可以是目录（递归）、文件或 glob
//! （`src/**/*.rs`）。`.git` / `node_modules` / `target` / `.dock` 下的改动不算，除非
//! 监听的就是那个目录。任务自己那一轮在跑时的改动也不算（[`FileWatches::hold`]）——
//! 不然它改文件又会触发自己。

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobMatcher};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::{CronJob, CronSchedule};

/// 这些目录下的改动默认不算（构建产物、依赖、版本库、Dock 自己的数据）。
const NOISY: [&str; 4] = [".git", "node_modules", "target", ".dock"];
/// 挂不上的过这么久再试。
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// 任务那一轮跑完后再静默这么久（它最后几下写盘的事件可能晚到）。
const SETTLE: Duration = Duration::from_secs(3);
/// 说明里最多列这么多个文件。
const LIST_MAX: usize = 50;

enum Rule {
    /// 这个目录下（含它自己）或就是这个文件。
    Under(PathBuf),
    Glob {
        root: PathBuf,
        matcher: GlobMatcher,
    },
}

impl Rule {
    fn root(&self) -> &Path {
        match self {
            Rule::Under(root) | Rule::Glob { root, .. } => root,
        }
    }

    fn matches(&self, path: &Path) -> bool {
        let root = self.root();
        let Ok(rel) = path.strip_prefix(root) else {
            return false;
        };
        let noisy = rel
            .components()
            .any(|c| matches!(c, Component::Normal(name) if NOISY.iter().any(|n| name == *n)));
        if noisy {
            return false;
        }
        match self {
            Rule::Under(_) => true,
            Rule::Glob { matcher, .. } => matcher.is_match(path),
        }
    }
}

#[derive(Default)]
struct Seen {
    paths: BTreeSet<PathBuf>,
    last: Option<Instant>,
}

struct Entry {
    /// 监听规格（工作目录 + 路径 + 防抖）；变了就重挂。
    key: String,
    cwd: PathBuf,
    debounce: Duration,
    seen: Arc<Mutex<Seen>>,
    _watchers: Vec<RecommendedWatcher>,
    /// 在这之前的改动不算（任务自己那一轮在跑）。
    quiet_until: Option<Instant>,
}

/// 一个任务攒够了的改动。
#[derive(Clone, Debug)]
pub struct FilesReady {
    pub id: String,
    /// 改了的文件（绝对路径，排好序）。
    pub paths: Vec<PathBuf>,
    pub cwd: PathBuf,
}

impl FilesReady {
    /// 写给模型的说明（英文，进 system-reminder）。
    pub fn context(&self) -> String {
        let shown: Vec<String> = self
            .paths
            .iter()
            .take(LIST_MAX)
            .map(|p| {
                let rel = p.strip_prefix(&self.cwd).unwrap_or(p);
                format!("- {}", rel.display())
            })
            .collect();
        let more = self.paths.len().saturating_sub(LIST_MAX);
        let tail = if more > 0 {
            format!("\n- … and {more} more")
        } else {
            String::new()
        };
        format!(
            "This run was triggered by file changes ({} file{}):\n{}{tail}",
            self.paths.len(),
            if self.paths.len() == 1 { "" } else { "s" },
            shown.join("\n")
        )
    }
}

/// 本进程挂着的文件监听。驱动每一轮 [`Self::reconcile`]，再取 [`Self::ready`]。
#[derive(Default)]
pub struct FileWatches {
    entries: HashMap<String, Entry>,
    /// 挂不上的：什么时候失败的、为什么。
    failed: HashMap<String, (String, Instant)>,
}

impl FileWatches {
    pub fn new() -> Self {
        Self::default()
    }

    /// 跟着任务表走：本进程持有的、没暂停、没跑完的文件变化任务挂上监听，其余摘掉。
    /// 回这一轮新挂不上的 `(任务 id, 原因)`，以及新挂上了的 id（原因要清掉）。
    pub fn reconcile(&mut self, jobs: &[CronJob]) -> (Vec<(String, String)>, Vec<String>) {
        let mut wanted: HashMap<&str, (&CronJob, String)> = HashMap::new();
        for job in jobs {
            let CronSchedule::Files(spec) = &job.schedule else {
                continue;
            };
            if !job.held_here || job.paused || job.done {
                continue;
            }
            let cwd = job_cwd(job);
            let key = format!(
                "{}|{:?}|{}",
                cwd.display(),
                spec.paths,
                spec.debounce.as_millis()
            );
            wanted.insert(job.id.as_str(), (job, key));
        }
        self.entries.retain(|id, entry| {
            wanted
                .get(id.as_str())
                .is_some_and(|(_, key)| *key == entry.key)
        });
        self.failed
            .retain(|id, (key, _)| wanted.get(id.as_str()).is_some_and(|(_, k)| k == key));
        let mut errors = Vec::new();
        let mut attached = Vec::new();
        for (id, (job, key)) in wanted {
            if self.entries.contains_key(id) {
                continue;
            }
            if self
                .failed
                .get(id)
                .is_some_and(|(_, at)| at.elapsed() < RETRY_AFTER)
            {
                continue;
            }
            let CronSchedule::Files(spec) = &job.schedule else {
                continue;
            };
            // 和事件里的路径同一种写法（macOS 的 `/var` 实际是 `/private/var`），说明里才能
            // 写成相对项目的路径。
            let cwd = job_cwd(job);
            let cwd = cwd.canonicalize().unwrap_or(cwd);
            match attach(&cwd, &spec.paths) {
                Ok((seen, watchers)) => {
                    let was_failed = self.failed.remove(id).is_some();
                    self.entries.insert(
                        id.to_string(),
                        Entry {
                            key,
                            cwd,
                            debounce: spec.debounce,
                            seen,
                            _watchers: watchers,
                            quiet_until: None,
                        },
                    );
                    if was_failed {
                        attached.push(id.to_string());
                    }
                }
                Err(e) => {
                    self.failed.insert(id.to_string(), (key, Instant::now()));
                    errors.push((id.to_string(), e));
                }
            }
        }
        (errors, attached)
    }

    /// 这个任务自己那一轮在跑：到现在为止（以及跑完后再等一会儿）的改动都不算。
    pub fn hold(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.quiet_until = Some(Instant::now() + SETTLE);
            *entry.seen.lock().unwrap() = Seen::default();
        }
    }

    /// 改动停了够久的任务：交出改了哪些文件，并清空。
    pub fn ready(&mut self) -> Vec<FilesReady> {
        let now = Instant::now();
        let mut out = Vec::new();
        for (id, entry) in &mut self.entries {
            let mut seen = entry.seen.lock().unwrap();
            if entry.quiet_until.is_some_and(|until| now < until) {
                *seen = Seen::default();
                continue;
            }
            entry.quiet_until = None;
            let Some(last) = seen.last else {
                continue;
            };
            if now.duration_since(last) < entry.debounce || seen.paths.is_empty() {
                continue;
            }
            let paths = std::mem::take(&mut seen.paths).into_iter().collect();
            seen.last = None;
            out.push(FilesReady {
                id: id.clone(),
                paths,
                cwd: entry.cwd.clone(),
            });
        }
        out
    }

    /// 挂着监听的任务数（测试 / 诊断用）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn job_cwd(job: &CronJob) -> PathBuf {
    job.owner
        .as_ref()
        .map(|o| o.cwd.clone())
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn expand(raw: &str, cwd: &Path) -> PathBuf {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// 一条路径变成规则：glob 的话取它前面不带通配的那段目录当根。根都要先 canonicalize
/// （macOS 的 FSEvents 报的是真实路径，`/var` 会变成 `/private/var`）。
fn rule_for(raw: &str, cwd: &Path) -> Result<Rule, String> {
    let full = expand(raw, cwd);
    let text = full.to_string_lossy().to_string();
    if !has_glob(&text) {
        let canon = full
            .canonicalize()
            .map_err(|e| format!("监听不了 {}：{e}", full.display()))?;
        return Ok(Rule::Under(canon));
    }
    let mut root = PathBuf::new();
    let mut rest = Vec::new();
    for comp in full.components() {
        let s = comp.as_os_str().to_string_lossy();
        if rest.is_empty() && !has_glob(&s) {
            root.push(comp);
        } else {
            rest.push(s.to_string());
        }
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("监听不了 {}：{e}", root.display()))?;
    let pattern = format!("{}/{}", root.display(), rest.join("/"));
    let matcher = GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("glob {raw:?} 写得不对：{e}"))?
        .compile_matcher();
    Ok(Rule::Glob { root, matcher })
}

type Attached = (Arc<Mutex<Seen>>, Vec<RecommendedWatcher>);

fn attach(cwd: &Path, paths: &[String]) -> Result<Attached, String> {
    let rules: Vec<Rule> = paths
        .iter()
        .map(|p| rule_for(p, cwd))
        .collect::<Result<_, _>>()?;
    let rules = Arc::new(rules);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut watchers = Vec::new();
    for rule in rules.iter() {
        let (target, mode) = match rule {
            // 单个文件：监听它所在的目录（编辑器常常是「写临时文件再改名」）。
            Rule::Under(path) if path.is_file() => (
                path.parent().unwrap_or(path).to_path_buf(),
                RecursiveMode::NonRecursive,
            ),
            other => (other.root().to_path_buf(), RecursiveMode::Recursive),
        };
        let sink = seen.clone();
        let all = rules.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else {
                return;
            };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let hits: Vec<PathBuf> = event
                .paths
                .into_iter()
                .filter(|p| all.iter().any(|r| r.matches(p)))
                .collect();
            if hits.is_empty() {
                return;
            }
            let mut seen = sink.lock().unwrap();
            seen.paths.extend(hits);
            seen.last = Some(Instant::now());
        })
        .map_err(|e| format!("文件监听起不来：{e}"))?;
        watcher
            .watch(&target, mode)
            .map_err(|e| format!("监听不了 {}：{e}", target.display()))?;
        watchers.push(watcher);
    }
    Ok((seen, watchers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::cron::{Cron, CronOwner, NewCronTask};

    fn files_job(cron: &Cron, cwd: &Path, paths: &[&str]) -> String {
        cron.create(NewCronTask {
            owner: Some(CronOwner {
                session: "s-1".into(),
                cwd: cwd.to_path_buf(),
            }),
            ..NewCronTask::new(
                CronSchedule::files(
                    paths.iter().map(|p| p.to_string()).collect(),
                    Duration::from_millis(300),
                )
                .unwrap(),
                "同步翻译",
            )
        })
        .unwrap()
    }

    async fn wait_ready(watches: &mut FileWatches) -> Vec<FilesReady> {
        for _ in 0..100 {
            let ready = watches.ready();
            if !ready.is_empty() {
                return ready;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Vec::new()
    }

    /// 改了监听的目录里的文件：防抖后交出来一次，带着改了哪些文件；不在规则里的不算。
    #[tokio::test]
    async fn changes_under_a_watched_glob_are_debounced_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(docs.join("node_modules")).unwrap();
        let cron = Cron::new();
        let id = files_job(&cron, dir.path(), &["docs/**/*.md"]);
        let mut watches = FileWatches::new();
        let (errors, _) = watches.reconcile(&cron.list());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(watches.len(), 1);
        // FSEvents 挂上要一点时间。
        tokio::time::sleep(Duration::from_millis(500)).await;

        std::fs::write(docs.join("a.md"), "一").unwrap();
        std::fs::write(docs.join("b.txt"), "不算").unwrap();
        std::fs::write(docs.join("node_modules").join("c.md"), "不算").unwrap();
        let ready = wait_ready(&mut watches).await;
        assert_eq!(ready.len(), 1, "等不到改动");
        assert_eq!(ready[0].id, id);
        let names: Vec<String> = ready[0]
            .paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.md".to_string()], "{:?}", ready[0].paths);
        assert!(
            ready[0].context().contains("- docs/a.md"),
            "{}",
            ready[0].context()
        );
        assert!(watches.ready().is_empty(), "交出来就清空");
    }

    /// 任务自己那一轮在跑时的改动不算；暂停 / 删掉的任务摘掉监听。
    #[tokio::test]
    async fn held_changes_are_dropped_and_paused_tasks_unwatched() {
        let dir = tempfile::tempdir().unwrap();
        let cron = Cron::new();
        let id = files_job(&cron, dir.path(), &["."]);
        let mut watches = FileWatches::new();
        watches.reconcile(&cron.list());
        tokio::time::sleep(Duration::from_millis(500)).await;
        std::fs::write(dir.path().join("self.txt"), "它自己写的").unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        watches.hold(&id);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(watches.ready().is_empty(), "跑着时的改动不算");

        cron.patch(
            &id,
            None,
            crate::tools::cron::CronPatch {
                paused: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        watches.reconcile(&cron.list());
        assert!(watches.is_empty(), "暂停了就摘掉");
    }

    #[test]
    fn a_missing_path_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let cron = Cron::new();
        let id = files_job(&cron, dir.path(), &["nope/"]);
        let mut watches = FileWatches::new();
        let (errors, _) = watches.reconcile(&cron.list());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, id);
        assert!(errors[0].1.contains("监听不了"), "{errors:?}");
        let (again, _) = watches.reconcile(&cron.list());
        assert!(again.is_empty(), "过一会儿再试，不是每一轮都报");
    }
}

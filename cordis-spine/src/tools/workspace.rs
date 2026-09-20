//! Workspace tools suite: `specs` / `handles` / `execute_with` dispatch.
//!
//! One capability per sibling dir (`read_file/`, `bash/`, …). Shared arg/path
//! helpers live in [`crate::tools::fs_common`]. Registration stays here so
//! permissions / plan gate / preset allowlists keep using `workspace_tools`.

use crate::tools::bash;
use crate::tools::glob;
use crate::tools::grep;
use crate::tools::jobs::Jobs;
use crate::tools::list_dir;
use crate::tools::read_file;
use crate::tools::search_replace;
use crate::tools::write_file;
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub fn specs() -> Vec<ToolSpec> {
    vec![
        list_dir::spec(),
        read_file::spec(),
        grep::spec(),
        search_replace::spec(),
        bash::spec(),
        glob::spec(),
        write_file::spec(),
    ]
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "list_dir"
            | "read_file"
            | "grep"
            | "search_replace"
            | "bash"
            | "run_terminal_cmd"
            | "glob"
            | "write_file"
    )
}

#[allow(dead_code)]
pub async fn execute(call: ToolCall) -> ToolResult {
    execute_with(call, || false, None).await
}

pub async fn execute_with(
    call: ToolCall,
    is_cancelled: impl Fn() -> bool + Send + Sync,
    jobs: Option<&Jobs>,
) -> ToolResult {
    if call.name == "read_file" {
        if let Some((content, images)) = read_file::maybe_image(&call.arguments) {
            return ToolResult {
                call_id: call.id,
                name: call.name,
                content,
                images: crate::tools::tool_images::cap_images(images),
            };
        }
    }
    let content = match call.name.as_str() {
        "list_dir" => list_dir::run(&call.arguments),
        "read_file" => read_file::run(&call.arguments),
        "grep" => grep::run(&call.id, &call.arguments).await,
        "search_replace" => search_replace::run(&call.arguments),
        "bash" | "run_terminal_cmd" => bash::run(&call.arguments, &is_cancelled, jobs).await,
        "glob" => glob::run(&call.arguments),
        "write_file" => write_file::run(&call.arguments),
        other => format!("unknown tool: {other}"),
    };
    ToolResult {
        call_id: call.id,
        name: call.name,
        content,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::execute;
    use crate::tools::bash::{self, FOREGROUND_MS_ENV};
    use crate::tools::jobs::Jobs;
    use cordis_base::types::ToolCall;

    #[tokio::test]
    async fn list_dir_json_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: format!(
                r#"{{"target_directory":{}}}"#,
                serde_json::to_string(&dir.path().to_string_lossy()).unwrap()
            ),
        };
        let out = execute(call).await;
        assert!(out.content.contains("a.txt"), "{}", out.content);
    }

    async fn replace(path: &std::path::Path, old: &str, new: &str, all: bool) -> String {
        let call = ToolCall {
            id: "1".into(),
            name: "search_replace".into(),
            arguments: serde_json::json!({
                "file_path": path,
                "old_string": old,
                "new_string": new,
                "replace_all": all,
            })
            .to_string(),
        };
        execute(call).await.content
    }

    #[tokio::test]
    async fn search_replace_unique_match_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let out = replace(&path, "alpha", "AAA", false).await;
        assert!(out.contains("updated"), "{out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "AAA beta gamma");
    }

    /// 原缺陷：`replace_all: false` 且 `old_string` 多处命中时，旧实现
    /// `replacen(..., 1)` **静默改掉第一处**——模型以为改的是自己瞄准的那处，
    /// 实际可能是文件里另一处同名代码。必须报错并回报命中数，且**一个字节都
    /// 不许写**。
    #[tokio::test]
    async fn search_replace_refuses_ambiguous_match_and_leaves_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        let before = "alpha beta alpha";
        std::fs::write(&path, before).unwrap();
        let out = replace(&path, "alpha", "AAA", false).await;
        assert!(out.contains("Error"), "多处命中必须报错：{out}");
        assert!(
            out.contains("2 次"),
            "要回报命中数，模型才知道该补多少上下文：{out}"
        );
        assert!(out.contains("replace_all"), "要给出逃生舱：{out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "报错路径上文件必须原封不动"
        );
    }

    /// 多处命中时 `replace_all: true` 是明示的逃生舱，要全改并说清改了几处。
    #[tokio::test]
    async fn search_replace_all_changes_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha beta alpha").unwrap();
        let out = replace(&path, "alpha", "AAA", true).await;
        assert!(out.contains("2 处"), "{out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "AAA beta AAA");
    }

    #[tokio::test]
    async fn search_replace_missing_old_string_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha").unwrap();
        let out = replace(&path, "zzz", "AAA", false).await;
        assert!(out.contains("未在"), "{out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
    }

    async fn run_glob(pattern: &str, dir: &std::path::Path) -> String {
        let call = ToolCall {
            id: "1".into(),
            name: "glob".into(),
            arguments: serde_json::json!({
                "glob_pattern": pattern,
                "target_directory": dir,
            })
            .to_string(),
        };
        execute(call).await.content
    }

    fn glob_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(dir.path().join("root.rs"), "x").unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "x").unwrap();
        std::fs::write(dir.path().join("src/deep/b.rs"), "x").unwrap();
        dir
    }

    /// 原缺陷一：手搓 matcher 把 `**/*.rs` 翻成 `^.*.*/.*\.rs$`，强制要有一个
    /// `/`，于是**根目录下的文件全被漏掉**——而 `**/*.rs` 正是模型最常打的模式。
    #[tokio::test]
    async fn glob_doublestar_includes_root_level_files() {
        let dir = glob_tree();
        let out = run_glob("**/*.rs", dir.path()).await;
        assert!(out.contains("root.rs"), "根目录文件不该被漏掉：{out}");
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(out.contains("src/deep/b.rs"), "{out}");
    }

    /// 原缺陷二：`*` 被翻成 `.*`，会跨过 `/`，于是 `src/*.rs` 把
    /// `src/deep/b.rs` 也匹配进来。globset 的 `literal_separator` 修掉这个。
    #[tokio::test]
    async fn glob_single_star_does_not_cross_separators() {
        let dir = glob_tree();
        let out = run_glob("src/*.rs", dir.path()).await;
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(!out.contains("deep/b.rs"), "`*` 不该跨过 `/`：{out}");
    }

    /// 无 `/` 的模式匹配任意深度的 basename（对齐 ripgrep / DSH）。
    #[tokio::test]
    async fn glob_bare_pattern_matches_basename_at_any_depth() {
        let dir = glob_tree();
        let out = run_glob("*.rs", dir.path()).await;
        for expected in ["root.rs", "src/a.rs", "src/deep/b.rs"] {
            assert!(out.contains(expected), "缺 {expected}：{out}");
        }
    }

    /// 原缺陷三：旧实现在判 `is_dir` 之前就把条目推进结果，目录混在文件里。
    #[tokio::test]
    async fn glob_returns_files_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/x.rs"), "x").unwrap();
        let out = run_glob("target", dir.path()).await;
        assert!(
            out.contains("no matches"),
            "`target` 是目录，不该作为结果返回：{out}"
        );
    }

    #[tokio::test]
    async fn glob_respects_gitignore() {
        let dir = glob_tree();
        std::fs::write(dir.path().join(".gitignore"), "src/deep/\n").unwrap();
        let out = run_glob("**/*.rs", dir.path()).await;
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(
            !out.contains("deep/b.rs"),
            "被 .gitignore 排除的不该出现：{out}"
        );
    }

    /// 空结果要说清楚搜了什么，并点出 `*` 不跨 `/` 这条最常见的踩坑。
    #[tokio::test]
    async fn glob_empty_result_explains_itself() {
        let dir = glob_tree();
        let out = run_glob("*.zzz", dir.path()).await;
        assert!(out.contains("no matches"), "{out}");
        assert!(out.contains("*.zzz"), "要回显模式：{out}");
    }

    /// `list_dir` 不该再把 `.gitignore` 排除掉的构建产物吐出来。
    #[tokio::test]
    async fn list_dir_filters_gitignored_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("keep.rs"), "x").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: serde_json::json!({ "target_directory": dir.path() }).to_string(),
        };
        let out = execute(call).await.content;
        assert!(out.contains("keep.rs"), "{out}");
        assert!(
            !out.contains("target/"),
            "被 .gitignore 排除的不该出现：{out}"
        );
    }

    /// 大目录收成计数 + 扩展名分布，不再逐条吐几千行。
    #[tokio::test]
    async fn list_dir_summarizes_large_directories() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..150 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "x").unwrap();
        }
        let call = ToolCall {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: serde_json::json!({ "target_directory": dir.path() }).to_string(),
        };
        let out = execute(call).await.content;
        assert!(out.contains("150 个文件"), "{out}");
        assert!(out.contains("rs × 150"), "要给扩展名分布：{out}");
        assert!(out.contains("另有 50 个文件未列出"), "{out}");
    }

    /// `workdir` 让模型不必写 `cd x && ...`。
    #[tokio::test]
    async fn bash_runs_in_workdir() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "30000");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let out = bash::run(
            &serde_json::json!({"command": "ls", "workdir": dir.path()}).to_string(),
            &|| false,
            None,
        )
        .await;
        assert!(out.contains("marker.txt"), "{out}");
    }

    /// 不存在的 `workdir` 要直接报错，而不是让命令在一个意外的目录里跑掉。
    #[tokio::test]
    async fn bash_rejects_missing_workdir() {
        let out = bash::run(
            r#"{"command":"echo hi","workdir":"/definitely/not/here"}"#,
            &|| false,
            None,
        )
        .await;
        assert!(out.contains("不是一个已存在的目录"), "{out}");
        assert!(!out.contains("hi"), "命令不该被执行：{out}");
    }

    /// `timeout_ms` 只能收紧：传一个很短的值应该提前收掉并带回已有输出。
    #[tokio::test]
    async fn bash_timeout_ms_tightens_the_budget() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "60000");
        let start = std::time::Instant::now();
        let out = bash::run(
            r#"{"command":"echo early; sleep 30","timeout_ms":600}"#,
            &|| false,
            None,
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "应按 timeout_ms 收紧，实际 {:?}",
            start.elapsed()
        );
        assert!(out.contains("early"), "超时也要带回已产出的输出：{out}");
    }

    /// `timeout_ms` 不能放宽超过前台预算上限，否则一条命令能把整轮挂住。
    #[tokio::test]
    async fn bash_timeout_ms_cannot_exceed_the_ceiling() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "500");
        let start = std::time::Instant::now();
        let jobs = Jobs::new();
        let out = bash::run(
            r#"{"command":"sleep 30","timeout_ms":600000}"#,
            &|| false,
            Some(&jobs),
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "不该被放宽到 10 分钟，实际 {:?}",
            start.elapsed()
        );
        assert!(out.contains("转入后台"), "预算到点要转后台：{out}");
    }

    #[tokio::test]
    async fn glob_finds_txt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "x").unwrap();
        std::fs::write(dir.path().join("skip.md"), "y").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "glob".into(),
            arguments: serde_json::json!({
                "glob_pattern": "*.txt",
                "target_directory": dir.path(),
            })
            .to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("keep.txt"), "{}", out.content);
        assert!(!out.content.contains("skip.md"), "{}", out.content);
    }

    #[tokio::test]
    async fn write_file_creates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let call = ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({
                "target_file": path,
                "contents": "hello",
            })
            .to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("wrote"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[tokio::test]
    async fn read_file_omitted_limit_caps_at_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let body: String = (1..=1_050).map(|i| format!("L{i}\n")).collect();
        std::fs::write(&path, &body).unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "target_file": path }).to_string(),
        };
        let out = execute(call).await;
        assert!(
            out.content.contains("1→L1\n"),
            "{}",
            &out.content[..80.min(out.content.len())]
        );
        assert!(
            out.content.contains("truncated"),
            "should note truncation: {}",
            out.content.lines().last().unwrap_or("")
        );
        assert!(
            out.content.contains("offset=1001"),
            "hint next offset: {}",
            out.content.lines().last().unwrap_or("")
        );
        assert!(!out.content.contains("L1050\n"), "must not dump past cap");
    }

    #[tokio::test]
    async fn read_file_explicit_limit_respected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({
                "target_file": path,
                "offset": 2,
                "limit": 2,
            })
            .to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("b\n"), "{}", out.content);
        assert!(out.content.contains("c\n"), "{}", out.content);
        assert!(!out.content.contains("d\n"), "{}", out.content);
        assert!(out.content.contains("offset=4"), "{}", out.content);
    }

    #[tokio::test]
    async fn read_file_small_file_no_truncation_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.txt");
        std::fs::write(&path, "only\n").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "target_file": path }).to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("1→only"), "{}", out.content);
        assert!(!out.content.contains("truncated"), "{}", out.content);
    }

    #[tokio::test]
    async fn read_file_returns_image_for_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend(std::iter::repeat_n(0u8, 40));
        std::fs::write(&path, &png).unwrap();
        let args = serde_json::json!({"target_file": path}).to_string();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: args,
        };
        let result = execute(call).await;
        assert!(
            result.content.contains("Image content included inline"),
            "{}",
            result.content
        );
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].mime, "image/png");
    }

    /// 前台 bash 的管道死锁：输出超过管道缓冲（64KB）就会把子进程堵死，
    /// 一条 0.1s 的命令要等满整个前台预算再被杀，输出还全丢。
    #[tokio::test]
    async fn bash_large_output_does_not_deadlock() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "3000");
        let start = std::time::Instant::now();
        let out = bash::run(
            r#"{"command":"yes OUTLINE0123456789 | head -20000; echo finished"}"#,
            &|| false,
            None,
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "0.1s 的命令不该耗满前台预算，实际 {:?}",
            start.elapsed()
        );
        assert!(
            out.contains("finished"),
            "收尾行应保留：{}",
            &out[..out.len().min(200)]
        );
    }

    /// 到了前台预算要把已经产出的输出带回来。旧实现直接 `child.kill()` 后返回
    /// 一句错误字符串，从不读管道，模型什么都拿不到。
    #[tokio::test]
    async fn bash_timeout_keeps_partial_output() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "700");
        let jobs = Jobs::new();
        let out = bash::run(
            r#"{"command":"echo early-line; sleep 30"}"#,
            &|| false,
            Some(&jobs),
        )
        .await;
        assert!(
            out.contains("early-line"),
            "超时也要带回已产出的输出：{out}"
        );
        assert!(out.contains("转入后台"), "要说明去向：{out}");
    }

    /// 前台预算到点**不杀进程**，转后台接着跑。
    ///
    /// 旧行为是 kill + 「需要跑完就用 is_background: true 重跑」：那条命令已经跑了
    /// 几分钟，杀掉等于把这几分钟扔了，重跑还要再花同样的时间、大概率再超时一次。
    #[tokio::test]
    async fn bash_timeout_backgrounds_instead_of_killing() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "500");
        let jobs = Jobs::new();
        let out = bash::run(
            r#"{"command":"echo before-budget; sleep 1.2; echo after-budget"}"#,
            &|| false,
            Some(&jobs),
        )
        .await;

        // 返回的不是错误：模型看到 `Error:` 会重试或换路子，而它什么都没做错。
        assert!(!out.contains("Error:"), "超时转后台不是错误：{out}");
        assert!(out.contains("before-budget"), "已产出的输出要带回：{out}");
        assert!(out.contains("不要重跑"), "要明说别重跑：{out}");

        // job_id 必须能对上一条还活着的任务，否则这条提示是空头支票。
        let id = out
            .lines()
            .find_map(|l| l.strip_prefix("job_id: "))
            .expect("要给出 job_id")
            .trim()
            .to_string();
        let snap = jobs.snapshot(&id).expect("任务还在表里");
        assert!(!snap.foreground, "转后台后不该再算前台：{id}");

        // 关键：进程没被杀，预算之后的那一行照样产出。
        let mut tail = String::new();
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            tail = jobs.snapshot(&id).map(|s| s.output).unwrap_or_default();
            if tail.contains("after-budget") {
                break;
            }
        }
        assert!(
            tail.contains("after-budget"),
            "转后台后命令应继续跑完，实际：{tail}"
        );
    }

    /// 取消（用户按 Esc）仍然是**杀掉**，不是转后台——那是明确要它停。
    #[tokio::test]
    async fn bash_cancel_still_kills() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "30000");
        let jobs = Jobs::new();
        let cancel_at = std::time::Instant::now() + Duration::from_millis(400);
        let out = bash::run(
            r#"{"command":"echo started; sleep 30"}"#,
            &|| std::time::Instant::now() >= cancel_at,
            Some(&jobs),
        )
        .await;
        assert!(out.contains("cancelled"), "取消要说明自己是取消：{out}");
        assert!(out.contains("started"), "已产出的输出仍要带回：{out}");
        assert!(!out.contains("job_id"), "取消不该留下后台任务：{out}");
        assert!(jobs.list().is_empty(), "取消后任务要摘掉");
    }

    /// 没挂 `"jobs"` 服务时不能转后台：本地表随调用一起析构，那个 job_id 没人
    /// 查得到。宁可退回 kill + 诚实报错，也不发空头支票。
    #[tokio::test]
    async fn bash_timeout_without_a_jobs_service_still_kills() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "500");
        let out = bash::run(r#"{"command":"echo only-line; sleep 30"}"#, &|| false, None).await;
        assert!(out.contains("only-line"), "已产出的输出要带回：{out}");
        assert!(
            out.contains("超时"),
            "没有可查的任务表就该照实报超时：{out}"
        );
        assert!(!out.contains("job_id"), "不该发无处可查的 job_id：{out}");
    }

    /// 前台命令必须在 `jobs` 里现身，否则 TUI 无处读它的实时输出；
    /// 结束后要摘掉，不能常驻。
    #[tokio::test]
    async fn bash_foreground_shows_live_progress_in_jobs() {
        // 同一进程里别的用例会改 DOCK_BASH_FOREGROUND_MS；不拿这把锁就会被它们的
        // 短预算污染，表现为本用例随机超时。
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "30000");
        let jobs = Jobs::new();
        let run = bash::run(
            r#"{"command":"for i in 1 2 3 4 5 6; do echo tick-$i; sleep 0.2; done"}"#,
            &|| false,
            Some(&jobs),
        );
        let watch = async {
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if jobs
                    .list()
                    .iter()
                    .any(|j| j.foreground && j.output.contains("tick-1"))
                {
                    return true;
                }
            }
            false
        };
        let (out, seen) = tokio::join!(run, watch);
        assert!(seen, "前台命令运行期间应能在 jobs 里看到它的实时输出");
        assert!(out.contains("tick-6"), "最终结果要完整：{out}");
        assert!(jobs.list().is_empty(), "前台任务结束后应从表里摘掉");
    }
}

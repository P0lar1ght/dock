//! 每轮改动：`search_replace` / `write_file` 改的文件按轮记下、算行数、出 diff、撤销；
//! 落盘后换一个实例照样读得出；撤回（rewind）切掉的轮次作废。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{
    agent_loop, install_core, permissions, settings, turn_changes, workspace_tools, AppSettings,
    BoxFuture, Llm, LlmOutput, LoopHandle, PermissionMode, PromptRequest, Sampler, Sessions,
    StreamDelta, ToolCall, TurnChanges, TurnChangesChanged, AGENT_LOOP, LLM, SESSIONS, SETTINGS,
    TURN_CHANGES, TURN_CHANGES_CHANGED,
};
use serde_json::json;

/// 每一轮先发排好的那批工具调用，再回一句话收尾。
#[derive(Default)]
struct Script {
    turns: Mutex<VecDeque<Vec<ToolCall>>>,
    /// 这一轮的工具已经发过了：下一次采样收尾。
    sent: Mutex<bool>,
}

impl Script {
    fn push(&self, calls: Vec<ToolCall>) {
        self.turns.lock().unwrap().push_back(calls);
    }
}

impl Sampler for Script {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        Box::pin(async move {
            let mut sent = self.sent.lock().unwrap();
            if !*sent {
                if let Some(calls) = self.turns.lock().unwrap().pop_front() {
                    *sent = true;
                    return LlmOutput {
                        tool_calls: calls,
                        ..LlmOutput::default()
                    };
                }
            }
            *sent = false;
            LlmOutput {
                text: "改好了".into(),
                ..LlmOutput::default()
            }
        })
    }
}

fn edit(id: &str, path: &Path, old: &str, new: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "search_replace".into(),
        arguments: json!({ "file_path": path, "old_string": old, "new_string": new }).to_string(),
    }
}

fn write(id: &str, path: &Path, contents: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "write_file".into(),
        arguments: json!({ "target_file": path, "contents": contents }).to_string(),
    }
}

async fn boot(script: Arc<Script>) -> Context {
    let root = Context::new();
    install_core(&root).await.unwrap();
    root.plugin(settings(), ()).unwrap().wait().await.unwrap();
    root.require::<AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Allow);
    root.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(cordis_spine::turn(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(workspace_tools(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(turn_changes(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.provide(LLM, Llm::from_sampler(root.clone(), script))
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root
}

async fn run(root: &Context, text: &str) {
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run(text)
        .await
        .unwrap();
}

/// 两轮各改一点：每轮的行数、diff 各算各的（第二轮的原文是第一轮改完的样子）；撤销只动
/// 还是那一轮改完样子的文件；落盘后另起一个实例读得出同样的结果。
#[tokio::test]
async fn edits_are_recorded_per_turn_and_revert_restores_what_is_still_ours() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let _env = cordis_base::test_env::scoped()
        .set("DOCK_HOME", home.path().join("dock-home"))
        .set("DOCK_CUA_DRIVER", "off")
        .cwd(&project);
    let a = project.join("a.txt");
    let fresh = project.join("notes/new.md");
    std::fs::write(&a, "one\ntwo\nthree\n").unwrap();

    let script = Arc::new(Script::default());
    let root = boot(script.clone()).await;
    root.require::<Sessions>(SESSIONS).unwrap().attach_disk();
    let events: Arc<Mutex<Vec<usize>>> = Arc::default();
    let _listen = {
        let events = events.clone();
        root.on(TURN_CHANGES_CHANGED, move |c: &TurnChangesChanged| {
            events.lock().unwrap().push(c.ordinal)
        })
        .unwrap()
    };

    script.push(vec![
        edit("c1", &a, "two", "TWO"),
        write("c2", &fresh, "x\ny\n"),
    ]);
    run(&root, "改一下").await;
    script.push(vec![edit("c3", &a, "one", "ONE")]);
    run(&root, "再改").await;

    let changes = root.require::<TurnChanges>(TURN_CHANGES).unwrap();
    let turns = changes.list(&root);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    let first = &turns[0];
    assert_eq!(first.ordinal, 0);
    assert_eq!((first.added, first.removed), (3, 1));
    let files: Vec<(&str, &str, usize, usize)> = first
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.status, f.added, f.removed))
        .collect();
    assert_eq!(
        files,
        vec![("a.txt", "modified", 1, 1), ("notes/new.md", "added", 2, 0)]
    );
    let second = &turns[1];
    assert_eq!(second.ordinal, 1);
    assert_eq!((second.added, second.removed), (1, 1));
    let diff = changes.diff(&root, 1).unwrap();
    assert!(
        diff.contains("-one\n+ONE\n TWO\n"),
        "第二轮的原文是第一轮改完的：{diff}"
    );
    let diff = changes.diff(&root, 0).unwrap();
    assert!(diff.contains("new file mode"), "{diff}");
    assert!(diff.contains("+++ b/notes/new.md"), "{diff}");
    assert!(
        events.lock().unwrap().contains(&0) && events.lock().unwrap().contains(&1),
        "改了要发变化：{:?}",
        events.lock().unwrap()
    );

    // 撤销第一轮：a.txt 之后又被第二轮改过，不动；新建的文件删掉。
    let outcome = changes.revert(&root, 0).unwrap();
    assert_eq!(outcome.restored, vec!["notes/new.md".to_string()]);
    assert_eq!(outcome.conflicts.len(), 1, "{outcome:?}");
    assert_eq!(outcome.conflicts[0].0, "a.txt");
    assert!(!fresh.exists());
    assert!(changes.revert(&root, 0).is_err(), "撤过的不能再撤");
    // 撤销第二轮：恢复成第二轮开始时的样子。
    let outcome = changes.revert(&root, 1).unwrap();
    assert_eq!(outcome.restored, vec!["a.txt".to_string()]);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\nTWO\nthree\n");

    // 落盘了：另起一个实例（同一棵树、同一个会话）读出来一样，撤销标记也在。
    let reread = TurnChanges::new(root.clone()).list(&root);
    assert_eq!(reread, changes.list(&root));
    assert!(reread.iter().all(|t| t.reverted), "{reread:#?}");
}

/// 撤回到第二条消息：它和之后的轮次作废；之后再发一条，同一个序号是新的一轮，不混进旧记录。
#[tokio::test]
async fn rewind_drops_the_turns_it_cuts() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let _env = cordis_base::test_env::scoped()
        .set("DOCK_HOME", home.path().join("dock-home"))
        .set("DOCK_CUA_DRIVER", "off")
        .cwd(&project);
    let a = project.join("a.txt");
    let b = project.join("b.txt");
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();

    let script = Arc::new(Script::default());
    let root = boot(script.clone()).await;
    script.push(vec![edit("c1", &a, "a", "A")]);
    run(&root, "一").await;
    script.push(vec![edit("c2", &b, "b", "B")]);
    run(&root, "二").await;
    let changes = root.require::<TurnChanges>(TURN_CHANGES).unwrap();
    assert_eq!(changes.list(&root).len(), 2);

    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .rewind_to_user(1)
        .unwrap();
    let turns = changes.list(&root);
    assert_eq!(turns.iter().map(|t| t.ordinal).collect::<Vec<_>>(), vec![0]);

    script.push(vec![edit("c3", &a, "A", "AA")]);
    run(&root, "换个说法").await;
    let turns = changes.list(&root);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    let paths: Vec<&str> = turns[1].files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt"], "旧的第二轮（b.txt）不该混进来");
}

/// 编辑跑到一半用户又发了一条消息（后台子代理、慢的编辑）：改动还归 pre 那一轮，不丢。
#[tokio::test]
async fn an_edit_spanning_a_new_user_message_stays_in_its_turn() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let _env = cordis_base::test_env::scoped()
        .set("DOCK_HOME", home.path().join("dock-home"))
        .set("DOCK_CUA_DRIVER", "off")
        .cwd(&project);
    let a = project.join("a.txt");
    std::fs::write(&a, "a\n").unwrap();

    let script = Arc::new(Script::default());
    let root = boot(script.clone()).await;
    run(&root, "一").await;
    let changes = root.require::<TurnChanges>(TURN_CHANGES).unwrap();
    let args = json!({ "file_path": &a }).to_string();
    changes.before_edit(&root, "search_replace", &args);
    run(&root, "二").await;
    std::fs::write(&a, "A\n").unwrap();
    changes.after_edit(&root, "search_replace");

    let turns = changes.list(&root);
    assert_eq!(
        turns.iter().map(|t| t.ordinal).collect::<Vec<_>>(),
        vec![0],
        "{turns:#?}"
    );
    assert_eq!((turns[0].added, turns[0].removed), (1, 1));
}

/// 一次编辑只重新看它自己改的那个文件：这一轮记过的别的文件不跟着重读。
#[tokio::test]
async fn after_edit_only_refreshes_the_file_it_edited() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let _env = cordis_base::test_env::scoped()
        .set("DOCK_HOME", home.path().join("dock-home"))
        .set("DOCK_CUA_DRIVER", "off")
        .cwd(&project);
    let a = project.join("a.txt");
    let b = project.join("b.txt");
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();

    let script = Arc::new(Script::default());
    let root = boot(script.clone()).await;
    run(&root, "一").await;
    let changes = root.require::<TurnChanges>(TURN_CHANGES).unwrap();
    changes.before_edit(
        &root,
        "write_file",
        &json!({ "target_file": &b }).to_string(),
    );
    changes.after_edit(&root, "write_file");
    // b.txt 在编辑之外被改了；接下来这次只改 a.txt，不该顺手把 b.txt 的新样子记进来。
    std::fs::write(&b, "B\n").unwrap();
    changes.before_edit(
        &root,
        "search_replace",
        &json!({ "file_path": &a }).to_string(),
    );
    std::fs::write(&a, "A\n").unwrap();
    changes.after_edit(&root, "search_replace");

    let turns = changes.list(&root);
    let paths: Vec<&str> = turns[0].files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt"], "{turns:#?}");
}

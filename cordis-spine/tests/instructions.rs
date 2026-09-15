//! `AGENTS.md` 走消息流而不是系统提示：接线层面的端到端。
//!
//! 单测覆盖了「渲染成什么」和「什么时候该注入」，但真正容易错的是**接线**：
//! `agent/step-start` 的 handler 拿不到调用方 ctx，靠循环挂的 task-local 才看得见
//! 当前这个 agent 的会话。这里跑真实的循环，看规约有没有落进历史。
//!
//! 单独一个测试二进制：它要往 `DOCK_HOME` 里写 `AGENTS.md`，而 `round.rs` 那边
//! 整个二进制共用一个 `DOCK_HOME`，混在一起会给它所有用例平白多注一条提醒。

use std::sync::Arc;

use cordis::Context;
use cordis_spine::{
    agent_loop, install_fakes, project_instructions, GrokStep, LogEvent, LoopHandle, Sessions,
    SESSIONS,
};

const RULE: &str = "本仓库的规约：改完必须跑 cargo test。";

/// 隔离的 `DOCK_HOME`，并在用户层放一份 `AGENTS.md`。
///
/// 用用户层而不是 `{cwd}/AGENTS.md`：改当前目录是进程级的，和同二进制里别的用例
/// 抢；`DOCK_HOME` 只要在建 Context 之前设好就够。
fn home_with_rules() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let path = tempfile::tempdir().unwrap().keep();
        std::env::set_var("DOCK_HOME", &path);
        std::env::set_var("DOCK_CUA_DRIVER", "off");
        std::fs::write(path.join("AGENTS.md"), RULE).unwrap();
        path
    })
}

fn reminders(sessions: &Sessions) -> Vec<String> {
    sessions
        .events()
        .into_iter()
        .filter_map(|e| match e {
            LogEvent::SystemReminder(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn agents_md_lands_in_the_history_not_the_system_prompt() {
    let home = home_with_rules();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    // 五件套里没有它（`install_app` 才挂），测接线就得自己挂上。
    root.plugin(project_instructions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    LoopHandle::new(root.clone(), Arc::new(GrokStep))
        .run("hello")
        .await
        .unwrap();

    let injected = reminders(&sessions);
    assert_eq!(injected.len(), 1, "规约没落进历史：{:?}", sessions.kinds());
    let body = &injected[0];
    assert!(body.starts_with("<system-reminder>"), "{body}");
    assert!(body.ends_with("</system-reminder>"), "{body}");
    assert!(body.contains(RULE), "{body}");
    assert!(
        body.contains(&home.join("AGENTS.md").display().to_string())
            || body.contains("~/.dock/AGENTS.md"),
        "要带来源标注：{body}"
    );

    // 位置：在用户那条之后、模型回答之前 —— 模型这一步就该看见规则。
    let kinds = sessions.kinds();
    let user = kinds.iter().position(|k| *k == "user").expect("user");
    let rule = kinds
        .iter()
        .position(|k| *k == "system-reminder")
        .expect("reminder");
    let reply = kinds
        .iter()
        .rposition(|k| *k == "llm/stream")
        .expect("assistant");
    assert!(user < rule && rule < reply, "{kinds:?}");

    // 系统提示这边一个字都不该有 —— 那正是这次搬家的目的。
    let system = root
        .require::<cordis_spine::SystemPrompt>(cordis_spine::SYSTEM_PROMPT)
        .unwrap()
        .assemble_on(&root);
    assert!(!system.contains(RULE), "规约仍在系统提示里：{system}");
}

/// 第二轮不该再注一份：文件没变，历史里那份还在。
#[tokio::test]
async fn a_second_turn_does_not_duplicate_the_copy() {
    home_with_rules();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    // 五件套里没有它（`install_app` 才挂），测接线就得自己挂上。
    root.plugin(project_instructions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    let loop_handle = LoopHandle::new(root.clone(), Arc::new(GrokStep));

    loop_handle.run("hello").await.unwrap();
    loop_handle.run("again").await.unwrap();

    assert_eq!(
        reminders(&sessions).len(),
        1,
        "文件没变却重复注入：{:?}",
        sessions.kinds()
    );
}

/// 子代理跑在隔离的 ctx 上，规约要按**它自己的**历史判断：拿错 ctx 的话，
/// handler 看的是主会话的历史，子代理就永远等不到自己那份。
#[tokio::test]
async fn a_child_isolate_gets_its_own_copy() {
    home_with_rules();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    // 五件套里没有它（`install_app` 才挂），测接线就得自己挂上。
    root.plugin(project_instructions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

    let parent = root.require::<Sessions>(SESSIONS).unwrap();
    LoopHandle::new(root.clone(), Arc::new(GrokStep))
        .run("hello")
        .await
        .unwrap();
    assert_eq!(reminders(&parent).len(), 1);

    let child_ctx = root.isolate("sessions");
    let child = Sessions::isolated_as(child_ctx.clone(), "child-1");
    let _hold = child_ctx.provide(SESSIONS, child.clone()).unwrap();
    LoopHandle::new(child_ctx.clone(), Arc::new(GrokStep))
        .run("子任务")
        .await
        .unwrap();

    assert_eq!(
        reminders(&child).len(),
        1,
        "子代理没拿到规约：{:?}",
        child.kinds()
    );
    assert!(reminders(&child)[0].contains(RULE));
    assert_eq!(reminders(&parent).len(), 1, "子代理不该往主会话里注东西");
}

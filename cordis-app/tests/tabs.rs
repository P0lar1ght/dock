//! 分页的组合根冒烟：真的按 `tab_mount()` 装一页出来，验会话是两份、
//! 全局服务仍是一份。

use cordis::Context;
use cordis_app::{session_actor, tab_mount};
use cordis_spine::{
    agent_loop, install_fakes, LlmOutput, LogEvent, Sessions, Tools, SESSIONS, TOOLS,
};
use cordis_tui::{
    prompt, tabs, theme, PromptWidget, SessionRef, TabKind, Tabs, SESSION_PORT, TUI_PROMPT,
    TUI_TABS,
};

/// 整个测试二进制共用一个隔离的 `DOCK_HOME`，免得读到本机 `~/.dock`。
fn isolated_home() {
    static HOME: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("dock-tabs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DOCK_HOME", &dir);
        std::env::set_var("DOCK_CUA_DRIVER", "off");
    });
}

async fn boot() -> Context {
    isolated_home();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.plugin(session_actor(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    // 第一页（根）的视图在真实启动里由 `tui()` 挂；这里只补测试要用到的两个。
    root.plugin(theme(), ()).unwrap().wait().await.unwrap();
    root.plugin(prompt(), ()).unwrap().wait().await.unwrap();
    root.plugin(tabs(), tab_mount())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root
}

#[tokio::test]
async fn a_new_tab_runs_its_own_session() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();

    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.append(LogEvent::User("主线的话".into()));

    tabs.open().await.unwrap();
    let tab_ctx = tabs.active_ctx();

    let tab_sessions = tab_ctx
        .get::<Sessions>(SESSIONS)
        .expect("分页要有自己的会话");
    assert!(
        tab_sessions.events().is_empty(),
        "空白新页不该带着主线的历史"
    );
    assert!(
        tab_sessions.is_main(),
        "分页是并列的主线，不是子代理（否则系统提示会按子代理裁）"
    );
    assert_ne!(tab_sessions.identity(), main.identity());

    // 反向也要成立：新页开出来没动过主会话。
    assert_eq!(main.events().len(), 1, "开分页不该碰主会话");

    // 每页各有一个会话 actor（排队 / working 是分开的）。
    assert!(tab_ctx.get::<SessionRef>(SESSION_PORT).is_some());

    // 没被 isolate 的名字照旧落回根：一张工具表的不变式还在。
    assert!(tab_ctx.get::<Tools>(TOOLS).is_some(), "工具表该落回根");
}

/// `/tab fork`：新页带着来源页的上下文**快照**，两边从此各写各的。
#[tokio::test]
async fn fork_snapshots_the_context_and_remembers_its_origin() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.append(LogEvent::User("怎么改这个 bug".into()));
    main.append(LogEvent::LlmStream(LlmOutput {
        text: "先看 config.rs".into(),
        ..Default::default()
    }));

    tabs.fork().await.unwrap();
    let fork_ctx = tabs.active_ctx();
    let fork_sessions = fork_ctx.get::<Sessions>(SESSIONS).unwrap();
    assert_eq!(
        fork_sessions.events().len(),
        2,
        "分叉页要带着来源页的上下文"
    );
    assert_eq!(
        tabs.list()[tabs.active_index()].origin,
        Some(1),
        "分叉页要记得来源"
    );

    // 快照是值传递：分叉之后两边互不影响。
    fork_sessions.append(LogEvent::User("只在分叉页说的话".into()));
    assert_eq!(main.events().len(), 2, "分叉页写字不该回流到来源页");
    main.append(LogEvent::User("只在主线说的话".into()));
    assert_eq!(
        fork_sessions.events().len(),
        3,
        "来源页写字也不该流到分叉页"
    );
}

#[tokio::test]
async fn forking_an_empty_page_is_refused() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    assert!(tabs.fork().await.is_err(), "空会话没什么可分叉的");
    assert_eq!(tabs.len(), 1, "被拒的分叉不该留下半页");
}

/// `Ctrl+B`：把分叉页的结论填进**来源页的输入框**并切过去 —— 不替用户发送，
/// 也不往来源页的历史里塞话。
#[tokio::test]
async fn carry_back_fills_the_origin_prompt_without_sending() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.append(LogEvent::User("怎么改这个 bug".into()));

    tabs.fork().await.unwrap();
    tabs.active_ctx()
        .get::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::LlmStream(LlmOutput {
            text: "结论：改 config.rs 第 42 行".into(),
            ..Default::default()
        }));

    let origin = tabs.carry_back().unwrap();
    assert_eq!(origin, 1);
    assert_eq!(tabs.active_index(), 0, "带回之后要切回来源页");

    let text = root.get::<PromptWidget>(TUI_PROMPT).unwrap().text();
    assert!(text.contains("结论：改 config.rs 第 42 行"), "{text}");
    assert!(text.contains("（来自第"), "{text}");
    assert_eq!(main.events().len(), 1, "带回只填输入框，不动来源页的历史");
}

#[tokio::test]
async fn carry_back_needs_a_fork() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    assert!(tabs.carry_back().is_err(), "第一页没有来源页");

    tabs.open().await.unwrap();
    assert!(
        tabs.carry_back().is_err(),
        "空白新页不是分叉出来的，没有来源页"
    );
}

/// `/btw`：旁问是**一张真分页**——有自己的滚动区（markdown / 工具卡照常渲染），
/// 带着当前页的快照、只读，且不进主线上下文。
#[tokio::test]
async fn aside_opens_a_read_only_tab() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.append(LogEvent::User("主线在干活".into()));

    let id = tabs.ask_aside("它现在卡在哪".into()).await.unwrap();

    // 进标签栏、切过去、标成只读。
    assert_eq!(tabs.len(), 2, "旁问就是一张分页");
    let info = tabs.list().into_iter().find(|t| t.id == id).unwrap();
    assert!(info.active, "问完就该切过去看答案");
    assert_eq!(info.kind, TabKind::Aside);
    assert_eq!(info.origin, Some(1));

    // 整套每页视图都挂上了（滚动区 / 输入框都在 `tab_mount` 那一串里），
    // 所以答案走的是正常渲染路径——markdown、工具卡、流式都在。
    let aside_ctx = tabs.active_ctx();
    assert!(aside_ctx.get::<PromptWidget>(TUI_PROMPT).is_some());
    assert!(aside_ctx.get::<Sessions>(SESSIONS).is_some());
    assert_eq!(main.events().len(), 1, "旁问不进主线上下文");
}

#[tokio::test]
async fn empty_question_is_refused() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    assert!(tabs.ask_aside("   ".into()).await.is_err());
    assert_eq!(tabs.len(), 1, "被拒的旁问不该留下半页");
}

/// 转正：拿旁问已经聊出来的历史开一张**全权**页，旁问那一页关掉。
#[tokio::test]
async fn promoting_an_aside_makes_a_full_tab() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    root.get::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::User("主线".into()));
    let aside_id = tabs.ask_aside("问一句".into()).await.unwrap();

    let id = tabs.promote_active().await.unwrap();
    assert_ne!(id, aside_id);
    assert_eq!(tabs.len(), 2, "旁问那一页要被关掉，换成转正的这张");
    let promoted = tabs.list().into_iter().find(|t| t.id == id).unwrap();
    assert_eq!(promoted.kind, TabKind::Normal, "转正之后是全权页");
    assert_eq!(promoted.origin, Some(1));
    assert!(
        !tabs.list().iter().any(|t| t.id == aside_id),
        "旁问页该没了"
    );
    assert!(
        !tabs
            .active_ctx()
            .get::<Sessions>(SESSIONS)
            .unwrap()
            .events()
            .is_empty(),
        "转正页要带着旁问聊过的历史"
    );
}

#[tokio::test]
async fn promoting_a_normal_tab_is_refused() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    assert!(tabs.promote_active().await.is_err(), "常驻页不用转正");
}

#[tokio::test]
async fn closing_a_tab_leaves_the_main_session_alone() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.append(LogEvent::User("主线的话".into()));

    tabs.open().await.unwrap();
    tabs.active_ctx()
        .get::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::User("第二页的话".into()));
    tabs.close(1).await.unwrap();

    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs.active_index(), 0);
    let events = main.events();
    assert_eq!(events.len(), 1, "关掉分页不该动主会话");
    assert!(matches!(&events[0], LogEvent::User(t) if t == "主线的话"));
}

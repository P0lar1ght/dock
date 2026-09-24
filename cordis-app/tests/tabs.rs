//! 分页的组合根冒烟：真的按 `tab_mount()` 装一页出来，验会话是两份、
//! 全局服务仍是一份。

use cordis::Context;
use cordis_app::{session_actor, tab_mount};
use cordis_spine::{
    agent_loop, clear_plan_for_session_switch, expected_plan_path, goal_service,
    goal_tool_registration, install_fakes, is_plan_file_edit, permissions, plan_mode_service,
    plan_mode_tool_registration, settings, todo_service, todo_tool_registration, AppSettings, Goal,
    LlmOutput, LogEvent, Mcp, PermissionMode, Permissions, PlanMode, PlanPhase, PreStep, Sessions,
    Todos, ToolCall, Tools, TurnEnd, AGENT_PRESETS, GOAL, MCP, PERMISSIONS, PLAN_MODE, PRE_STEP,
    SESSIONS, SETTINGS, TODOS, TOOLS, TURN, TURN_END,
};
use cordis_tui::{
    prompt, tabs, theme, PromptWidget, SessionRef, TabKind, Tabs, SESSION_PORT, TUI_PROMPT,
    TUI_TABS,
};

/// 每个测试自己一个隔离的 `DOCK_HOME`，免得读到本机 `~/.dock`，也免得
/// 并行跑的测试抢同一个分页计划目录（分页目录按页号命名，两页都叫 `main#2`）。
fn isolated_home() {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("dock-tabs-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DOCK_HOME", &dir);
    std::env::set_var("DOCK_CUA_DRIVER", "off");
}

/// `boot()` 的结果：根上下文 + 串行锁。解引用成 `Context`，用例照常 `root.get(..)`。
///
/// 为什么要串行：同一个测试二进制里的用例**共用 pid**，而 `DOCK_HOME` 是进程级
/// 环境变量（`isolated_home` 后设的会盖掉先设的），所以分页的临时计划目录
/// `sessions/<cwd>/tabs/<pid>/main#2` 实际上是大家共用的。任何一个用例开第 2 页都会
/// `discard_ephemeral_plan` 删掉它——别的用例刚写下的 `plan.md` 就没了。生产里一个
/// 进程只有一个 `"tui.tabs"`，不存在这种并发，所以这里让开页的用例一个一个来，而
/// 不是改产品代码或放宽断言。
struct Booted {
    root: Context,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for Booted {
    type Target = Context;

    fn deref(&self) -> &Context {
        &self.root
    }
}

async fn boot() -> Booted {
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let serial = SERIAL.lock().await;
    isolated_home();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    // 第 1 页就是根。分页子树由 `tab_mount` 再挂一份，这里补上根上那一份。
    root.plugin(goal_service(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(goal_tool_registration(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(todo_service(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(todo_tool_registration(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(plan_mode_service(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(plan_mode_tool_registration(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
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
    Booted {
        root,
        _serial: serial,
    }
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

/// 历史会话开成自己的一页：当前页留着，这一页能接着收消息，再开一次只是切回去。
#[tokio::test]
async fn opening_history_adds_a_tab_you_can_talk_to() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    assert!(
        tabs.open_archived("missing").await.is_err(),
        "没有这份会话不该留下半页"
    );
    assert_eq!(tabs.len(), 1);

    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.attach_disk();
    main.append(LogEvent::User("历史里的那句".into()));
    let id = main.archive_current().unwrap().id;
    main.append(LogEvent::User("当前页还在".into()));
    assert_ne!(main.live_session_id(), id);

    let tab_id = tabs.open_archived(&id).await.unwrap();
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs.active_id(), tab_id);
    assert_eq!(tabs.active_index(), 1);

    let page = tabs.active_ctx();
    let opened = page.get::<Sessions>(SESSIONS).unwrap();
    assert_eq!(opened.live_session_id(), id);
    assert_eq!(opened.identity(), "main#2");
    assert!(
        opened
            .events()
            .iter()
            .any(|e| matches!(e, LogEvent::User(t) if t == "历史里的那句")),
        "新页要带着那份历史"
    );
    assert!(page.get::<SessionRef>(SESSION_PORT).is_some());
    assert!(page.get::<PromptWidget>(TUI_PROMPT).is_some());

    page.get::<SessionRef>(SESSION_PORT)
        .unwrap()
        .submit("接着说".into(), true);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if opened
                .events()
                .iter()
                .any(|e| matches!(e, LogEvent::User(t) if t == "接着说"))
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("这一页自己的循环该收下这句话");
    page.get::<SessionRef>(SESSION_PORT).unwrap().cancel();

    assert!(
        main.events()
            .iter()
            .any(|e| matches!(e, LogEvent::User(t) if t == "当前页还在")),
        "当前页还在"
    );
    assert!(
        !main
            .events()
            .iter()
            .any(|e| matches!(e, LogEvent::User(t) if t == "接着说")),
        "发给新页的话不该进当前页"
    );

    let again = tabs.open_archived(&id).await.unwrap();
    assert_eq!(again, tab_id, "已经开着就切过去，不再复制一页");
    assert_eq!(tabs.len(), 2);
}

/// 切页之后底栏读的是这一页的模型、协议和权限模式；批准框也只挂在这一页的队列上。
#[tokio::test]
async fn each_page_keeps_its_model_mode_and_permission_queue() {
    let root = boot().await;
    root.plugin(settings(), ()).unwrap().wait().await.unwrap();
    root.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let main = root.get::<AppSettings>(SETTINGS).unwrap();
    main.set_model("page-one");
    main.set_permission_mode(PermissionMode::Ask);

    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    tabs.open().await.unwrap();
    let page = tabs.active_ctx();
    let second = page.get::<AppSettings>(SETTINGS).unwrap();
    assert_eq!(second.model(), "page-one", "新页从当前页抄一份");
    second.set_model("page-two");
    second.set_permission_mode(PermissionMode::Allow);
    assert_eq!(main.model(), "page-one");
    assert_eq!(main.permission_mode(), PermissionMode::Ask);
    assert_eq!(second.permission_mode(), PermissionMode::Allow);
    // 始终允许不会入队。下面要看的是队列隔离，先回到询问。
    second.set_permission_mode(PermissionMode::Ask);

    let first_q = root.get::<Permissions>(PERMISSIONS).unwrap();
    let second_q = page.get::<Permissions>(PERMISSIONS).unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&first_q, &second_q),
        "两页不该共用一个权限队列"
    );
    let waiting = std::sync::Arc::clone(&second_q);
    tokio::spawn(async move {
        waiting.request("bash", "只属于第二页").await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if second_q.front().is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("第二页的权限请求该进第二页的队列");
    assert!(first_q.front().is_none(), "第一页不该看见第二页的批准框");
    assert_eq!(second_q.front().unwrap().summary, "只属于第二页");
}

/// 关页要清掉那一页的 MCP elicitation：队列是全局的，fiber dispose 带不走它，
/// 不 cancel 的话那条工具调用永远等不到答复。
#[tokio::test]
async fn closing_a_tab_cancels_its_pending_elicitation() {
    let root = boot().await;
    root.plugin(cordis_spine::mcp_client(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let mcp = root.get::<Mcp>(MCP).unwrap();
    let elicit = mcp.elicitation();

    tabs.open().await.unwrap();
    let page = tabs.active_ctx();
    let page_name = page
        .get::<Sessions>(SESSIONS)
        .and_then(|s| s.ui_page())
        .expect("分页有自己的页名");

    // 这一页正在跑的 MCP 工具发来的提问
    let _scope = elicit.scope_page(Some(page_name.clone()));
    let queued = elicit.clone();
    let created = tokio::spawn(async move {
        queued
            .create("fake-server", serde_json::json!({"message": "选哪个？"}))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while elicit.front_for(Some(&page_name)).is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("提问该排进这一页的队列");

    tabs.close(1).await.unwrap();

    assert!(
        elicit.front_for(Some(&page_name)).is_none(),
        "关页该带走这一页的提问"
    );
    let value = tokio::time::timeout(std::time::Duration::from_secs(3), created)
        .await
        .expect("关页该给那条调用一个答复")
        .unwrap();
    assert!(
        value
            .get("action")
            .is_some_and(|a| a.as_str() == Some("cancel")),
        "提问被取消，工具调用才不会挂住: {value}"
    );
}

/// `/resume` 恢复时，若别的页正 live 在这份会话上，切过去而不是再开一份：
/// 两页同时往同一个 `chat_history.jsonl` 追加会损坏历史。
#[tokio::test]
async fn restoring_an_id_live_on_another_tab_switches_there() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let main = root.get::<Sessions>(SESSIONS).unwrap();
    main.attach_disk();
    main.append(LogEvent::User("历史里的那句".into()));
    let id = main.archive_current().unwrap().id;

    let tab_id = tabs.open_archived(&id).await.unwrap();
    assert_eq!(tabs.len(), 2, "开一份历史会话该多一页");

    // 回到第一页，模拟 /resume 选中同一个 id：不复制，只切到开着的那一页
    tabs.activate(0);
    assert_eq!(
        tabs.switch_to_live_session(&id),
        Some(tab_id),
        "已经开着就该切过去"
    );
    assert_eq!(tabs.active_id(), tab_id);
    assert_eq!(tabs.len(), 2, "不该再开一页");
    assert!(
        tabs.switch_to_live_session("unknown-id").is_none(),
        "没开着的会话不该被当成已开"
    );
}

fn turn_end(identity: &str) -> TurnEnd {
    TurnEnd::new("说完了", 0, true, false, identity)
}

fn pre_step(identity: &str) -> PreStep {
    PreStep::new("继续", true, identity)
}

/// `Ctrl+N` 开出来的第二页，和主线的目标、待办、计划互不续跑、互不覆盖。
///
/// 走的是 `tab_mount` 那条真分页，不是手搓两份服务。
#[tokio::test]
async fn two_pages_keep_goal_todos_and_plan_apart() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
    let tools = root.get::<Tools>(TOOLS).unwrap();

    let probe = Sessions::tab(root.clone(), 2);
    let stale = expected_plan_path(Some(&probe));
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "上一轮的计划").unwrap();

    tabs.open().await.unwrap();
    assert!(
        !stale.exists(),
        "开页要丢掉上一进程留下的 main#2 计划: {stale:?}"
    );

    let page = tabs.active_ctx();
    let page_sessions = page.get::<Sessions>(SESSIONS).unwrap();
    assert_eq!(page_sessions.identity(), "main#2");

    root.get::<Goal>(GOAL).unwrap().start("只属于第一页");
    let end = turn_end("main#2");
    let out = root.waterfall(TURN_END, end.clone(), || end);
    assert!(
        out.decision().is_none(),
        "第 2 页收尾不能被第 1 页的目标续跑: {:?}",
        out.decision()
    );
    let end = turn_end("main");
    let out = root.waterfall(TURN_END, end.clone(), || end);
    assert!(out.decision().is_some(), "第 1 页自己的目标还要续跑");

    let step = pre_step("main#2");
    root.waterfall(PRE_STEP, step.clone(), || step);
    assert!(
        root.get::<Goal>(GOAL).unwrap().take_instruction().is_some(),
        "第 2 页的 pre-step 不能吃掉第 1 页的一次性目标指令"
    );

    let listed = tools
        .execute_on(
            &root,
            ToolCall {
                id: "p1".into(),
                name: "todo_write".into(),
                arguments:
                    r#"{"merge":false,"todos":[{"id":"p1","content":"第一页","status":"pending"}]}"#
                        .into(),
            },
        )
        .await;
    assert!(!listed.content.starts_with("Error"), "{}", listed.content);
    let end = turn_end("main#2");
    let out = root.waterfall(TURN_END, end.clone(), || end);
    assert!(
        out.decision().is_none(),
        "第 1 页未完成的待办不能逼第 2 页续跑: {:?}",
        out.decision()
    );

    let listed = tools
        .execute_on(
            &page,
            ToolCall {
                id: "p2".into(),
                name: "todo_write".into(),
                arguments:
                    r#"{"merge":false,"todos":[{"id":"p2","content":"第二页","status":"pending"}]}"#
                        .into(),
            },
        )
        .await;
    assert!(!listed.content.starts_with("Error"), "{}", listed.content);
    assert_eq!(root.get::<Todos>(TODOS).unwrap().snapshot().len(), 1);
    assert_eq!(page.get::<Todos>(TODOS).unwrap().snapshot().len(), 1);
    assert_eq!(page.get::<Todos>(TODOS).unwrap().snapshot()[0].0, "p2");

    root.get::<PlanMode>(PLAN_MODE).unwrap().enter_pending();
    let step = pre_step("main#2");
    root.waterfall(PRE_STEP, step.clone(), || step);
    assert_eq!(
        root.get::<PlanMode>(PLAN_MODE).unwrap().phase(),
        PlanPhase::Pending,
        "第 2 页开一轮不能把第 1 页的计划推进成 Active"
    );
    clear_plan_for_session_switch(&root);
    assert_eq!(
        root.get::<PlanMode>(PLAN_MODE).unwrap().phase(),
        PlanPhase::Inactive
    );

    let entered = tools
        .execute_on(
            &page,
            ToolCall {
                id: "plan".into(),
                name: "enter_plan_mode".into(),
                arguments: "{}".into(),
            },
        )
        .await;
    assert!(entered.content.contains("main#2"), "{}", entered.content);
    let path = expected_plan_path(Some(page_sessions.as_ref()));
    assert!(path.exists(), "计划该写到 {path:?}");
    assert!(
        entered
            .content
            .contains(&path.to_string_lossy().to_string()),
        "{}",
        entered.content
    );
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let tabs_at = comps.iter().position(|c| c == "tabs").expect("{path:?}");
    assert_ne!(comps[tabs_at - 1], "sessions", "cwd-key 被丢掉了: {path:?}");
    let args = format!(r#"{{"target_file":"{}"}}"#, path.display());
    assert!(is_plan_file_edit("write_file", &args, &path));
    assert!(!is_plan_file_edit(
        "write_file",
        r#"{"target_file":".dock/plan.md"}"#,
        &path
    ));
    assert_eq!(
        root.get::<PlanMode>(PLAN_MODE).unwrap().phase(),
        PlanPhase::Inactive,
        "第 2 页进入计划模式不能打开第 1 页的写门"
    );
    assert_eq!(
        page.get::<PlanMode>(PLAN_MODE).unwrap().phase(),
        PlanPhase::Active
    );

    let child = page.isolate(SESSIONS).isolate(TURN).isolate(AGENT_PRESETS);
    child
        .provide(
            SESSIONS,
            Sessions::isolated_as(child.clone(), "child-from-2"),
        )
        .unwrap();
    let listed = tools
        .execute_on(
            &child,
            ToolCall {
                id: "c".into(),
                name: "todo_write".into(),
                arguments:
                    r#"{"merge":true,"todos":[{"id":"c","content":"孩子写的","status":"pending"}]}"#
                        .into(),
            },
        )
        .await;
    assert!(!listed.content.starts_with("Error"), "{}", listed.content);
    assert_eq!(
        root.get::<Todos>(TODOS).unwrap().snapshot().len(),
        1,
        "第 2 页的孩子不能改第 1 页的待办"
    );
    assert_eq!(page.get::<Todos>(TODOS).unwrap().snapshot().len(), 2);

    tabs.close(1).await.unwrap();
    assert!(!path.exists(), "关页要删掉这一页的计划文件: {path:?}");
}

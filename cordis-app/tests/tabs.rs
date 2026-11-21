//! 分页的组合根冒烟：真的按 `tab_mount()` 装一页出来，验会话是两份、
//! 全局服务仍是一份。

use cordis::Context;
use cordis_app::{session_actor, tab_mount};
use cordis_spine::{
    agent_loop, clear_plan_for_session_switch, expected_plan_path, goal_service,
    goal_tool_registration, install_fakes, is_plan_file_edit, permissions, plan_mode_service,
    plan_mode_tool_registration, settings, todo_service, todo_tool_registration, tool_task,
    AgentPresets, AppSettings, Goal, LlmOutput, LogEvent, Mcp, PermissionMode, Permissions,
    PlanMode, PlanPhase, PreStep, Sessions, Subagents, TaskConfig, Todos, ToolCall, Tools, TurnEnd,
    AGENT_PRESETS, GOAL, MCP, PERMISSIONS, PLAN_MODE, PRE_STEP, SESSIONS, SETTINGS, SUBAGENTS,
    TODOS, TOOLS, TURN, TURN_END,
};
use cordis_spine::{tabs, SessionRef, TabKind, Tabs, SESSION_PORT, TABS};
use cordis_tui::{carry_back, prompt, theme, PromptWidget, TUI_PROMPT};

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
/// 进程只有一个 `"tabs"`，不存在这种并发，所以这里让开页的用例一个一个来，而
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
    // 第一页（根）的视图在真实启动里由组合根（`main` 里的 `views()` 循环）挂；
    // 这里只补测试要用到的两个。
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
    let tabs = root.get::<Tabs>(TABS).unwrap();

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
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
    assert!(tabs.fork().await.is_err(), "空会话没什么可分叉的");
    assert_eq!(tabs.len(), 1, "被拒的分叉不该留下半页");
}

/// `Ctrl+B`：把分叉页的结论填进**来源页的输入框**并切过去 —— 不替用户发送，
/// 也不往来源页的历史里塞话。
#[tokio::test]
async fn carry_back_fills_the_origin_prompt_without_sending() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
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

    let origin = carry_back(&tabs).unwrap();
    assert_eq!(origin, 1);
    assert_eq!(tabs.active_index(), 0, "带回之后要切回来源页");

    let text = root.get::<PromptWidget>(TUI_PROMPT).unwrap().text();
    assert!(text.contains("结论：改 config.rs 第 42 行"), "{text}");
    assert!(text.contains("（来自第"), "{text}");
    assert_eq!(main.events().len(), 1, "带回只填输入框，不动来源页的历史");
}

/// TUI 的视图按页各一份，靠组合根经 `TabsConfig::per_tab` 把它们加进隔离名单。
/// 漏了这一步，第 2 页挂 `prompt()` 时撞上根上那份（「service `tui.prompt` has been
/// registered」），开页直接失败；这里同时钉住开得出来、且不是同一个输入框。
#[tokio::test]
async fn each_page_gets_its_own_views() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let page = tabs.active_ctx();
    let root_prompt = root.get::<PromptWidget>(TUI_PROMPT).unwrap();
    let page_prompt = page.get::<PromptWidget>(TUI_PROMPT).unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&root_prompt, &page_prompt),
        "第 2 页的输入框不该是第 1 页那个"
    );
}

#[tokio::test]
async fn carry_back_needs_a_fork() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
    assert!(carry_back(&tabs).is_err(), "第一页没有来源页");

    tabs.open().await.unwrap();
    assert!(
        carry_back(&tabs).is_err(),
        "空白新页不是分叉出来的，没有来源页"
    );
}

/// `/btw`：旁问是**一张真分页**——有自己的滚动区（markdown / 工具卡照常渲染），
/// 带着当前页的快照、只读，且不进主线上下文。
#[tokio::test]
async fn aside_opens_a_read_only_tab() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
    assert!(tabs.ask_aside("   ".into()).await.is_err());
    assert_eq!(tabs.len(), 1, "被拒的旁问不该留下半页");
}

/// 转正：拿旁问已经聊出来的历史开一张**全权**页，旁问那一页关掉。
#[tokio::test]
async fn promoting_an_aside_makes_a_full_tab() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
    assert!(tabs.promote_active().await.is_err(), "常驻页不用转正");
}

#[tokio::test]
async fn closing_a_tab_leaves_the_main_session_alone() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
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

    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
        .unwrap()
        .expect("提问该有答复");
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    let tabs = root.get::<Tabs>(TABS).unwrap();
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
    // 待办在回合末投的是记账票，`decision()` 对它本来就回 `None`；真正挡住
    // 「第 1 页的清单让第 2 页记账」的是这一条。
    assert!(
        out.decision_tools().is_none(),
        "第 1 页未完成的待办不能让第 2 页记账: {:?}",
        out.decision_tools()
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
    // 常驻页落盘：计划在这一页**自己的**会话目录里（和主会话同一种布局），
    // 不再是 `tabs/<pid>/main#2` 那个临时目录（现在只有 `/btw` 旁问页用它）。
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
    let n = comps.len();
    assert_eq!(
        comps[n - 2],
        page_sessions.live_session_id(),
        "计划该在第 2 页自己的会话目录里: {path:?}"
    );
    assert_ne!(comps[n - 3], "sessions", "cwd-key 被丢掉了: {path:?}");
    assert_ne!(
        path,
        expected_plan_path(root.get::<Sessions>(SESSIONS).as_deref()),
        "两页不能共用一个计划文件"
    );
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

    // 常驻页是落盘的会话：关页只是不再开着，计划跟会话一起留在磁盘上，
    // `/resume` 回来还在（和主会话一样）。临时计划目录只有旁问页有，关页时删。
    tabs.close(1).await.unwrap();
    assert!(
        path.exists(),
        "关页后计划该留在这一页的会话目录里: {path:?}"
    );
}

/// 回归：预设和工作目录都按页。新页从当前页抄一份预设、继承它钉住的 cwd；之后
/// 第 2 页切预设不改第 1 页——以前常驻页共用根上那一份。
#[tokio::test]
async fn a_new_page_forks_presets_and_inherits_the_cwd() {
    let root = boot().await;
    let home = std::path::PathBuf::from(std::env::var("DOCK_HOME").unwrap());
    root.provide(AGENT_PRESETS, AgentPresets::load(home.join("presets")))
        .unwrap();
    let first = root.get::<AgentPresets>(AGENT_PRESETS).unwrap();
    first.apply("cordis").unwrap();
    let project = home.join("project-b");
    std::fs::create_dir_all(&project).unwrap();
    root.get::<Sessions>(SESSIONS)
        .unwrap()
        .pin_workspace_cwd(&project);

    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let page = tabs.active_ctx();

    let second = page.get::<AgentPresets>(AGENT_PRESETS).unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&first, &second),
        "两页不该共用一份预设"
    );
    assert_eq!(second.current_id(), "cordis", "新页从当前页抄当前预设");
    second.apply("warden").unwrap();
    assert_eq!(first.current_id(), "cordis", "第 1 页不该跟着切");

    assert_eq!(
        cordis_spine::session_cwd(&page),
        project,
        "新页继承当前页的 cwd"
    );
}

/// 回归：常驻页落盘，而且落在**这一页自己的** cwd 下（以前分页不落盘，退出即
/// 丢）；关页后还在历史里。旁问页（`/btw`）是一次性的插话，不落盘。
#[tokio::test]
async fn resident_pages_persist_under_their_own_cwd() {
    let root = boot().await;
    let home = std::path::PathBuf::from(std::env::var("DOCK_HOME").unwrap());
    let project = home.join("project-c");
    std::fs::create_dir_all(&project).unwrap();
    root.get::<Sessions>(SESSIONS)
        .unwrap()
        .pin_workspace_cwd(&project);

    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let page = tabs.active_ctx();
    let sessions = page.get::<Sessions>(SESSIONS).unwrap();
    assert!(sessions.on_disk(), "常驻页该落盘");
    sessions.append(LogEvent::User("第二页说的话".into()));
    let id = sessions.live_session_id();
    assert!(
        Sessions::can_adopt_archived(&id, &project),
        "该落在这一页的 cwd（{project:?}）下"
    );
    let process_cwd = std::env::current_dir().unwrap();
    assert!(
        !Sessions::can_adopt_archived(&id, &process_cwd),
        "不该落到进程 cwd 下"
    );

    tabs.close(1).await.unwrap();
    assert!(
        Sessions::can_adopt_archived(&id, &project),
        "关页后会话还在历史里"
    );

    tabs.ask_aside("顺便问一句".into()).await.unwrap();
    let aside = tabs.active_ctx().get::<Sessions>(SESSIONS).unwrap();
    assert!(!aside.on_disk(), "旁问页不落盘");
}

/// 关掉一页：子代理信箱里按页记的唤醒一起丢掉（否则开开关关的分页让这张表只增不减）。
/// 关页后再要那一页的唤醒，拿到的是新建的。
#[tokio::test]
async fn closing_a_tab_forgets_its_subagent_mailbox() {
    let root = boot().await;
    root.plugin(tool_task(), TaskConfig::default())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let sub = root.get::<Subagents>(SUBAGENTS).unwrap();
    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let page = tabs
        .active_ctx()
        .get::<Sessions>(SESSIONS)
        .unwrap()
        .identity()
        .to_string();
    let wake = sub.parent_wake(&page);
    assert!(std::sync::Arc::ptr_eq(&wake, &sub.parent_wake(&page)));

    tabs.close(1).await.unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&wake, &sub.parent_wake(&page)),
        "关页后唤醒表里不该还留着那一页"
    );
}

/// 等到 `cond` 成立（定时任务驱动一秒一跳）。
async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..80 {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("等不到：{what}");
}

fn said(sessions: &Sessions, text: &str) -> bool {
    sessions
        .events()
        .iter()
        .any(|e| matches!(e, LogEvent::User(t) if t == text))
}

/// 定时任务送进它所属的那一页，不是第一页；会话关着就在后台把它开成一页再送。
#[tokio::test]
async fn scheduled_prompts_reach_their_own_session_even_when_closed() {
    let root = boot().await;
    let project = std::env::temp_dir().join(format!("dock-cron-proj-{}", std::process::id()));
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let _cron = root
        .provide(cordis_spine::CRON, cordis_spine::Cron::new())
        .unwrap();
    root.plugin(cordis_app::cron_driver(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let cron = root.get::<cordis_spine::Cron>(cordis_spine::CRON).unwrap();
    let tabs = root.get::<Tabs>(TABS).unwrap();
    let main = root.get::<Sessions>(SESSIONS).unwrap();

    let page = tabs.open_at(&project, None).await.unwrap();
    let sessions = page.get::<Sessions>(SESSIONS).unwrap();
    let id = sessions.live_session_id();
    assert!(!id.is_empty());
    let owner = cordis_spine::CronOwner {
        session: id.clone(),
        cwd: project.clone(),
    };
    cron.add_owned(
        Some(owner.clone()),
        std::time::Duration::from_secs(600),
        "第二页的定时提问",
        true,
    )
    .unwrap();
    eventually("送进第二页", || said(&sessions, "第二页的定时提问")).await;
    assert!(!said(&main, "第二页的定时提问"), "不该送进第一页");
    eventually("第二页落盘", || {
        Sessions::can_adopt_archived(&id, &project)
    })
    .await;

    tabs.close_session(&id).await.unwrap();
    cron.add_owned(
        Some(owner),
        std::time::Duration::from_secs(600),
        "关着时的定时提问",
        true,
    )
    .unwrap();
    eventually("会话被重新开成一页", || {
        tabs.contexts().iter().any(|ctx| {
            ctx.get::<Sessions>(SESSIONS)
                .is_some_and(|s| s.live_session_id() == id && said(&s, "关着时的定时提问"))
        })
    })
    .await;
    assert!(!said(&main, "关着时的定时提问"), "不该送进第一页");
    assert!(cron.list().iter().all(|j| j.last_error.is_none()));
}

/// 最多等 `secs` 秒（跑真命令、等文件监听的用例比 `eventually` 慢）。
async fn eventually_within(secs: u64, what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("等不到：{what}");
}

fn reminded(sessions: &Sessions, needle: &str) -> bool {
    sessions
        .events()
        .iter()
        .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains(needle)))
}

/// 定时任务用的项目目录 + 一页会话 + 挂好的驱动。
async fn schedule_fixture(name: &str) -> (Booted, std::path::PathBuf, Context) {
    let root = boot().await;
    let project = std::env::temp_dir().join(format!("dock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    std::mem::forget(
        root.provide(cordis_spine::CRON, cordis_spine::Cron::new())
            .unwrap(),
    );
    root.plugin(cordis_app::cron_driver(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let page = root
        .get::<Tabs>(TABS)
        .unwrap()
        .open_at(&project, None)
        .await
        .unwrap();
    (root, project, page)
}

fn owner_of(page: &Context, project: &std::path::Path) -> cordis_spine::CronOwner {
    cordis_spine::CronOwner {
        session: page.get::<Sessions>(SESSIONS).unwrap().live_session_id(),
        cwd: project.to_path_buf(),
    }
}

/// 「每次新开会话」：每跑一次在同一个项目里开一个新会话送进去，不进建任务的那个会话；
/// 提醒里写明是专门开的、带上手动运行附的话；历史记下每次送进了哪个会话。
#[tokio::test]
async fn a_new_session_task_opens_a_fresh_session_for_each_run() {
    let (root, project, page) = schedule_fixture("cron-fresh").await;
    let cron = root.get::<cordis_spine::Cron>(cordis_spine::CRON).unwrap();
    let owner = owner_of(&page, &project);
    let id = cron
        .create(cordis_spine::NewCronTask {
            owner: Some(owner.clone()),
            target: cordis_spine::CronTarget::NewSession,
            ..cordis_spine::NewCronTask::new(cordis_spine::CronSchedule::Manual, "写今天的日报")
        })
        .unwrap();
    let page_of = |session: &str| {
        root.get::<Tabs>(TABS)
            .unwrap()
            .contexts()
            .into_iter()
            .find(|p| {
                p.get::<Sessions>(SESSIONS)
                    .is_some_and(|s| s.live_session_id() == session)
            })
    };

    let mut runs = Vec::new();
    for input in ["周四", "周五"] {
        cron.trigger(
            &id,
            None,
            cordis_spine::FireReason::Manual,
            Some(format!("Input passed to this run:\n{input}")),
        )
        .unwrap();
        let want = runs.len() + 1;
        eventually_within(10, "送进一个新会话", || {
            cron.get(&id).unwrap().history.len() == want
        })
        .await;
        let run = cron.get(&id).unwrap().history.last().cloned().unwrap();
        assert_eq!(run.outcome, cordis_spine::RunOutcome::Sent, "{run:?}");
        let session = run.session.clone().unwrap();
        assert_ne!(session, owner.session, "不进建任务的那个会话");
        let fresh = page_of(&session).expect("新会话开成了一页");
        let sessions = fresh.get::<Sessions>(SESSIONS).unwrap();
        eventually("提问送进新会话", || said(&sessions, "写今天的日报")).await;
        assert!(reminded(&sessions, "opened just for this run"));
        assert!(reminded(&sessions, input), "带上手动运行附的话");
        let port = fresh.get::<SessionRef>(SESSION_PORT).unwrap();
        eventually("这一轮跑完", || !port.working() && !port.has_queued()).await;
        runs.push(session);
    }
    assert_ne!(runs[0], runs[1], "每次一个新会话");
    assert!(!said(
        &page.get::<Sessions>(SESSIONS).unwrap(),
        "写今天的日报"
    ));
}

/// 条件命令「成功时」：驱动马上在项目目录里跑一次命令，成功了就送进会话，提醒里带着
/// 命令和它的输出（模型据此知道发生了什么），记成一次条件触发。
#[tokio::test]
async fn a_watch_that_succeeds_fires_with_the_command_output() {
    let (root, project, page) = schedule_fixture("cron-watch").await;
    std::fs::write(project.join("deploy.status"), "v1.2 已上线\n").unwrap();
    let cron = root.get::<cordis_spine::Cron>(cordis_spine::CRON).unwrap();
    let id = cron
        .create(cordis_spine::NewCronTask {
            owner: Some(owner_of(&page, &project)),
            ..cordis_spine::NewCronTask::new(
                cordis_spine::CronSchedule::watch(
                    "cat deploy.status",
                    std::time::Duration::from_secs(60),
                    cordis_spine::WatchWhen::Succeeds,
                )
                .unwrap(),
                "部署好了，跑一遍冒烟测试",
            )
        })
        .unwrap();
    let sessions = page.get::<Sessions>(SESSIONS).unwrap();
    eventually_within(15, "条件满足后送进会话", || {
        said(&sessions, "部署好了，跑一遍冒烟测试")
    })
    .await;
    assert!(reminded(
        &sessions,
        "`cat deploy.status` now succeeds (exit code 0)"
    ));
    assert!(reminded(&sessions, "v1.2 已上线"));
    eventually("记进历史", || {
        !cron.get(&id).unwrap().history.is_empty()
    })
    .await;
    let job = cron.get(&id).unwrap();
    assert_eq!(job.runs, 1);
    assert_eq!(job.history[0].reason, cordis_spine::FireReason::Watch);
    assert!(job
        .watch
        .as_ref()
        .is_some_and(|w| w.matched && w.exit == Some(0)));
}

/// 文件变化：监听的目录里有改动，安静下来后送进会话，提醒里列出改了哪些文件
/// （相对项目的路径）。
#[tokio::test]
async fn a_file_change_fires_with_the_changed_files() {
    let (root, project, page) = schedule_fixture("cron-files").await;
    std::fs::create_dir_all(project.join("notes")).unwrap();
    let cron = root.get::<cordis_spine::Cron>(cordis_spine::CRON).unwrap();
    let id = cron
        .create(cordis_spine::NewCronTask {
            owner: Some(owner_of(&page, &project)),
            ..cordis_spine::NewCronTask::new(
                cordis_spine::CronSchedule::files(
                    vec!["notes".into()],
                    std::time::Duration::from_secs(1),
                )
                .unwrap(),
                "整理一下新笔记",
            )
        })
        .unwrap();
    // 驱动下一轮挂上监听，FSEvents 生效还要一点时间。
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(cron.get(&id).unwrap().last_error.is_none());
    std::fs::write(project.join("notes").join("周会.md"), "# 周会").unwrap();
    let sessions = page.get::<Sessions>(SESSIONS).unwrap();
    eventually_within(15, "改动安静下来后送进会话", || {
        said(&sessions, "整理一下新笔记")
    })
    .await;
    assert!(
        reminded(&sessions, "- notes/周会.md"),
        "{:?}",
        sessions.events()
    );
    eventually("记进历史", || {
        !cron.get(&id).unwrap().history.is_empty()
    })
    .await;
    assert_eq!(
        cron.get(&id).unwrap().history[0].reason,
        cordis_spine::FireReason::Files
    );
}

/// 一页关掉（不管谁关的）就发 `tabs/page-closed`，带着它的身份和会话 id：网关据此告诉
/// 各端这个线程关了。
#[tokio::test]
async fn closing_a_page_announces_it() {
    let root = boot().await;
    let closed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = closed.clone();
    let _listener = root
        .on(
            cordis_spine::TABS_PAGE_CLOSED,
            move |c: &cordis_spine::PageClosed| sink.lock().unwrap().push(c.clone()),
        )
        .unwrap();
    let project = std::env::temp_dir().join(format!("dock-close-proj-{}", std::process::id()));
    std::fs::create_dir_all(&project).unwrap();
    let tabs = root.get::<Tabs>(TABS).unwrap();
    let page = tabs.open_at(&project, None).await.unwrap();
    let sessions = page.get::<Sessions>(SESSIONS).unwrap();
    let (identity, id) = (sessions.identity().to_string(), sessions.live_session_id());
    assert!(!tabs.session_in_use(&id), "后台开的空闲页不算在用");
    tabs.close_session(&id).await.unwrap();
    assert_eq!(
        closed.lock().unwrap().clone(),
        vec![cordis_spine::PageClosed {
            identity,
            session: id
        }]
    );
}

/// 手动跑一次，等第 `n` 次运行记进历史，回它送进的会话。
async fn run_by_hand(cron: &cordis_spine::Cron, id: &str, n: usize) -> String {
    cron.trigger(id, None, cordis_spine::FireReason::Manual, None)
        .unwrap();
    eventually_within(10, "送进新会话", || {
        cron.get(id).unwrap().history.len() == n
    })
    .await;
    cron.get(id)
        .unwrap()
        .history
        .last()
        .unwrap()
        .session
        .clone()
        .unwrap()
}

/// 驱动替定时任务开出来的页（每次新开的会话）闲置够久就关掉，不然每跑一次终端里多一个分页；
/// 正看着的那一页、建任务的会话（不是驱动开的）不关。会话留在盘上。
#[tokio::test]
async fn pages_the_driver_opened_are_closed_once_idle() {
    let root = boot().await;
    let project = std::env::temp_dir().join(format!("dock-cron-idle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    std::mem::forget(
        root.provide(cordis_spine::CRON, cordis_spine::Cron::new())
            .unwrap(),
    );
    root.plugin(
        cordis_app::cron_driver_closing_after(std::time::Duration::from_millis(800)),
        (),
    )
    .unwrap()
    .wait()
    .await
    .unwrap();
    let tabs = root.get::<Tabs>(TABS).unwrap();
    let owner_page = tabs.open_at(&project, None).await.unwrap();
    let owner = owner_of(&owner_page, &project);
    let cron = root.get::<cordis_spine::Cron>(cordis_spine::CRON).unwrap();
    let id = cron
        .create(cordis_spine::NewCronTask {
            owner: Some(owner.clone()),
            target: cordis_spine::CronTarget::NewSession,
            ..cordis_spine::NewCronTask::new(cordis_spine::CronSchedule::Manual, "写今天的日报")
        })
        .unwrap();
    let open = |session: &str| {
        tabs.contexts().iter().any(|p| {
            p.get::<Sessions>(SESSIONS)
                .is_some_and(|s| s.live_session_id() == session)
        })
    };
    let first = run_by_hand(&cron, &id, 1).await;
    assert!(open(&first));
    eventually_within(10, "闲置后关掉", || !open(&first)).await;
    assert!(
        Sessions::can_adopt_archived(&first, &project),
        "会话还在盘上"
    );

    // 正看着的那一页不关；切走以后再关。
    let second = run_by_hand(&cron, &id, 2).await;
    let index = tabs
        .contexts()
        .iter()
        .position(|p| {
            p.get::<Sessions>(SESSIONS)
                .is_some_and(|s| s.live_session_id() == second)
        })
        .unwrap();
    assert!(tabs.activate(index));
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(open(&second), "正看着的不关");
    assert!(tabs.activate(0));
    eventually_within(10, "切走后关掉", || !open(&second)).await;
    assert!(open(&owner.session), "建任务的会话不是驱动开的，不关");
}

/// 一直在忙的一页：队列由测试摆好，记下驱动提交了什么。
#[derive(Default)]
struct BusyPort {
    queued: std::sync::Mutex<Vec<cordis_spine::QueuedItem>>,
    submitted: std::sync::Mutex<Vec<String>>,
}

impl cordis_spine::SessionPort for BusyPort {
    fn submit(&self, text: String, _send_now: bool) {
        self.submitted.lock().unwrap().push(text);
    }
    fn steer(&self, _text: String, _images: Vec<cordis_spine::UserImage>) {}
    fn steer_queued(&self, _id: Option<String>) {}
    fn merge_side_note(&self, _note: String) {}
    fn working(&self) -> bool {
        true
    }
    fn cancel(&self) {}
    fn has_queued(&self) -> bool {
        !self.queued.lock().unwrap().is_empty()
    }
    fn compact(&self, _context: String) {}
    fn queued_prompts(&self) -> Vec<cordis_spine::QueuedItem> {
        self.queued.lock().unwrap().clone()
    }
    fn promote(&self, _id: Option<String>) {}
    fn take_queued(&self, _id: Option<String>) -> Option<cordis_spine::QueuedItem> {
        None
    }
}

/// 回归：那一页一直在忙、上一次触发还排在队里没轮到时，到点不再叠一条一样的。
/// 旧驱动每次都 `submit`，一轮跑得比间隔长，同一句提问就在队列里越积越多，
/// 忙完一口气连跑好几遍。别的任务照常送达。
#[tokio::test]
async fn a_fire_whose_previous_run_is_still_queued_is_not_stacked() {
    let root = Context::new();
    let port = std::sync::Arc::new(BusyPort::default());
    port.queued.lock().unwrap().push(cordis_spine::QueuedItem {
        id: "prompt-1".into(),
        text: "看一眼 CI".into(),
        images: Vec::new(),
    });
    let _port = root
        .provide(SESSION_PORT, SessionRef::new(port.clone()))
        .unwrap();
    let _cron = root
        .provide(cordis_spine::CRON, cordis_spine::Cron::new())
        .unwrap();
    let cron = root.get::<cordis_spine::Cron>(cordis_spine::CRON).unwrap();
    let minute = std::time::Duration::from_secs(60);
    let stacked = cron.add_ex(minute, "看一眼 CI", true).unwrap();
    cron.add_ex(minute, "另一件事", true).unwrap();
    root.plugin(cordis_app::cron_driver(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();

    eventually("另一个任务照常送达", || {
        port.submitted
            .lock()
            .unwrap()
            .iter()
            .any(|t| t == "另一件事")
    })
    .await;
    eventually("两个都触发过、记进历史", || {
        cron.list().iter().all(|j| j.history.len() == 1)
    })
    .await;
    let submitted = port.submitted.lock().unwrap().clone();
    assert_eq!(
        submitted,
        vec!["另一件事".to_string()],
        "排着的那条不该再叠"
    );
    let job = cron.get(&stacked).unwrap();
    assert!(job.last_error.is_none());
    assert_eq!(
        job.history[0].outcome,
        cordis_spine::RunOutcome::Coalesced,
        "跳过的那次记成合并掉，不是送达"
    );
}

/// 网关给**不是当前页**的会话开侧边聊天：只读旁问页从那个会话抄设置（模型）和
/// 上下文，当前页不动；不是从正在看的那一页抄。
#[tokio::test]
async fn an_aside_for_another_page_inherits_that_page() {
    let root = boot().await;
    root.plugin(settings(), ()).unwrap().wait().await.unwrap();
    root.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let parent = tabs.active_ctx();
    parent
        .get::<AppSettings>(SETTINGS)
        .unwrap()
        .set_model("parent-model");
    let parent_sessions = parent.get::<Sessions>(SESSIONS).unwrap();
    parent_sessions.append(LogEvent::User("主线说的话".into()));
    tabs.activate(0);
    root.get::<AppSettings>(SETTINGS)
        .unwrap()
        .set_model("active-model");

    let (aside, existing) = tabs
        .open_aside_for(&parent_sessions.live_session_id())
        .await
        .unwrap();
    assert!(!existing);
    assert_eq!(tabs.active_index(), 0, "不切页");
    assert_eq!(
        aside.get::<AppSettings>(SETTINGS).unwrap().model(),
        "parent-model",
        "从来源页抄，不是从当前页"
    );
    let aside_sessions = aside.get::<Sessions>(SESSIONS).unwrap();
    assert!(
        aside_sessions
            .events()
            .iter()
            .any(|e| matches!(e, LogEvent::User(t) if t == "主线说的话")),
        "带着来源页的上下文"
    );
    let presets = aside.get::<AgentPresets>(AGENT_PRESETS).unwrap();
    assert!(presets.current().read_only, "侧边聊天是只读的");
}

/// `/tab merge`：只在旁问页里用；写进来源页的上下文（来源页闲着就直接落进历史，
/// 不开新的一轮），旁问页自己不多一条。
#[tokio::test]
async fn tab_merge_writes_the_note_into_the_origin() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
    assert!(
        cordis_tui::merge_note(&tabs, "x").is_err(),
        "主线页里不能 merge"
    );
    // 来源页用一张按 `tab_mount` 装出来的页（有自己的会话 actor）。
    tabs.open().await.unwrap();
    let origin_id = tabs.active_id();
    let main = tabs.active_ctx().get::<Sessions>(SESSIONS).unwrap();
    main.append(LogEvent::User("主线".into()));
    tabs.ask_aside("顺便问一句".into()).await.unwrap();
    let aside = tabs.active_ctx().get::<Sessions>(SESSIONS).unwrap();

    let origin = cordis_tui::merge_note(&tabs, "retry 要退避").unwrap();
    assert_eq!(origin, origin_id);
    eventually("笔记落进来源页", || {
        main.events().iter().any(|e| {
            matches!(e, LogEvent::SystemReminder(t)
                if cordis_spine::side_note_text(t).as_deref() == Some("retry 要退避"))
        })
    })
    .await;
    assert!(
        !main.events().iter().any(|e| matches!(e, LogEvent::PreStep)),
        "不开新的一轮：{:?}",
        main.events()
    );
    assert!(
        !aside
            .events()
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(t)
            if cordis_spine::side_note_text(t).is_some())),
        "笔记不写进旁问页"
    );
    let too_long = "字".repeat(cordis_spine::SIDE_NOTE_MAX_CHARS + 1);
    assert!(cordis_tui::merge_note(&tabs, &too_long).is_err());
}

/// TUI `/tab close` 关掉来源页：它的侧边聊天（网关开的）和 `/btw` 旁问页一起关，
/// 不留孤儿。以前只有网关 `thread/close` 级联，TUI 关页后侧边聊天还挂着。
#[tokio::test]
async fn closing_a_page_closes_its_asides() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let parent_id = tabs.active_id();
    let parent = tabs.active_ctx().get::<Sessions>(SESSIONS).unwrap();
    parent.append(LogEvent::User("主线".into()));
    let (side, _) = tabs
        .open_aside_for(&parent.live_session_id())
        .await
        .unwrap();
    let side_id = side.get::<Sessions>(SESSIONS).unwrap().live_session_id();
    tabs.ask_aside("顺便问一句".into()).await.unwrap();
    assert_eq!(tabs.len(), 4);

    tabs.close_id(Some(parent_id)).await.unwrap();
    assert_eq!(tabs.len(), 1, "来源页关了，旁问页一起关：{:?}", tabs.list());
    assert!(!tabs.is_aside(&side_id));
    assert_eq!(tabs.active_index(), 0);
}

/// 来源页 `/new`（`Ctrl+W`）换了会话：之前那张侧边聊天带的是旧会话的快照，
/// 不再算这一页的侧边聊天——网关再开要开新的，`/tab merge` 不能把笔记写进新会话。
#[tokio::test]
async fn an_aside_does_not_follow_its_page_into_a_new_session() {
    let root = boot().await;
    let tabs = root.get::<Tabs>(TABS).unwrap();
    tabs.open().await.unwrap();
    let parent = tabs.active_ctx().get::<Sessions>(SESSIONS).unwrap();
    parent.append(LogEvent::User("旧会话".into()));
    let old_id = parent.live_session_id();
    let (old_side, _) = tabs.open_aside_for(&old_id).await.unwrap();
    tabs.ask_aside("顺便问一句".into()).await.unwrap();
    let btw = tabs.active_index();

    // `/new` 的那两步。
    parent.archive_current();
    parent.clear();
    parent.append(LogEvent::User("新会话".into()));
    let new_id = parent.live_session_id();
    assert_ne!(new_id, old_id);

    assert!(tabs.aside_of(&new_id).is_none(), "旧的侧边聊天不归新会话");
    let (new_side, existing) = tabs.open_aside_for(&new_id).await.unwrap();
    assert!(!existing);
    assert_ne!(
        new_side
            .get::<Sessions>(SESSIONS)
            .unwrap()
            .live_session_id(),
        old_side
            .get::<Sessions>(SESSIONS)
            .unwrap()
            .live_session_id()
    );

    tabs.activate(btw);
    let err = cordis_tui::merge_note(&tabs, "旧结论").unwrap_err();
    assert!(err.contains("换了会话"), "{err}");
    assert!(
        !parent.events().iter().any(|e| matches!(e,
            LogEvent::SystemReminder(t) if cordis_spine::side_note_text(t).is_some())),
        "笔记不该进新会话"
    );
}

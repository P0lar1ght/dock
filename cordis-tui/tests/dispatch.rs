use cordis_tui::{dispatch, Action, Effect, PromptWidget};

#[test]
fn send_prompt_emits_effect() {
    let prompt = PromptWidget::default();
    prompt.push('h');
    let effects = dispatch(Action::SendPrompt("hello".into()), &prompt);
    assert!(matches!(
        effects.as_slice(),
        [Effect::SendPrompt {
            text,
            send_now: false
        }] if text == "hello"
    ));
    assert!(prompt.text().is_empty());
}

#[test]
fn empty_prompt_is_dropped() {
    let prompt = PromptWidget::default();
    let effects = dispatch(Action::SendPrompt("  ".into()), &prompt);
    assert!(effects.is_empty());
}

#[test]
fn slash_submit_maps_to_effect() {
    let prompt = PromptWidget::default();
    let effects = dispatch(Action::SendPrompt("/quit".into()), &prompt);
    assert!(matches!(effects.as_slice(), [Effect::Quit]));
}

#[test]
fn slash_accept_inserts_highlighted_row() {
    let prompt = PromptWidget::default();
    prompt.insert_str("/q");
    let effects = dispatch(Action::SlashAccept, &prompt);
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(prompt.text(), "/quit ");
}

#[test]
fn slash_plan_tasks_mcps_map() {
    let prompt = PromptWidget::default();
    let plan = dispatch(Action::SendPrompt("/plan".into()), &prompt);
    assert!(matches!(
        plan.as_slice(),
        [Effect::EnterPlan { description: None }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/view-plan".into()), &prompt).as_slice(),
        [Effect::ViewPlan]
    ));
    let plan_desc = dispatch(Action::SendPrompt("/plan refactor auth".into()), &prompt);
    assert!(matches!(
        plan_desc.as_slice(),
        [Effect::EnterPlan { description: Some(d) }] if d == "refactor auth"
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/tasks".into()), &prompt).as_slice(),
        [Effect::ShowTasks]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/workflow".into()), &prompt).as_slice(),
        [Effect::ToggleWorkflows]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/workflow runs".into()), &prompt).as_slice(),
        [Effect::ToggleWorkflows]
    ));
    match dispatch(
        Action::SendPrompt("/workflow deep-research why rust".into()),
        &prompt,
    )
    .as_slice()
    {
        [Effect::RunTool {
            name,
            arguments,
            title,
        }] => {
            assert_eq!(name, "workflow");
            assert_eq!(title, "/deep-research");
            assert!(arguments.contains("deep-research"), "{arguments}");
            assert!(arguments.contains("why rust"), "{arguments}");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        dispatch(Action::SendPrompt("/mcps".into()), &prompt).as_slice(),
        [Effect::ShowMcps]
    ));
    prompt.insert_str("/lsp st");
    let filled = dispatch(Action::SlashAccept, &prompt);
    assert!(filled.is_empty(), "{filled:?}");
    assert_eq!(prompt.text(), "/lsp status ");
    prompt.clear();
    assert!(matches!(
        dispatch(Action::SendPrompt("/lsp".into()), &prompt).as_slice(),
        [Effect::ShowLsp {
            write: true,
            user: false
        }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/lsp status".into()), &prompt).as_slice(),
        [Effect::ShowLsp {
            write: false,
            user: false
        }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/lsp user".into()), &prompt).as_slice(),
        [Effect::ShowLsp {
            write: true,
            user: true
        }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/lsp nope".into()), &prompt).as_slice(),
        [Effect::FillPrompt { text }] if text.contains("/lsp")
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/cordis".into()), &prompt).as_slice(),
        [Effect::ShowCordis]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/plugins".into()), &prompt).as_slice(),
        [Effect::ShowCordis]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/preset".into()), &prompt).as_slice(),
        [Effect::ShowPresets { focus: None }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/agent daily-sec".into()), &prompt).as_slice(),
        [Effect::ShowPresets { focus: Some(id) }] if id == "daily-sec"
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/usage".into()), &prompt).as_slice(),
        [Effect::ShowUsage]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/cost".into()), &prompt).as_slice(),
        [Effect::ShowUsage]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/context".into()), &prompt).as_slice(),
        [Effect::ShowContext]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/compact".into()), &prompt).as_slice(),
        [Effect::Compact { context }] if context.is_empty()
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/compact keep the test".into()), &prompt).as_slice(),
        [Effect::Compact { context }] if context == "keep the test"
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/goal".into()), &prompt).as_slice(),
        [Effect::FillPrompt { text }] if text.contains("用法") && text.contains("/goal")
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/goal ship the lsp tool".into()), &prompt).as_slice(),
        [Effect::EnterGoal { objective: Some(d) }] if d == "ship the lsp tool"
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/goal status".into()), &prompt).as_slice(),
        [Effect::ShowGoal { editing: false }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/goal pause".into()), &prompt).as_slice(),
        [Effect::GoalPause]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/loop".into()), &prompt).as_slice(),
        [Effect::FillPrompt { text }] if text.contains("用法") && text.contains("/loop")
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/loop 5m check deploy".into()), &prompt).as_slice(),
        [Effect::EnterLoop { args }] if args == "5m check deploy"
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/loop check deploy every hour".into()), &prompt).as_slice(),
        [Effect::EnterLoop { args }] if args == "check deploy every hour"
    ));
}

#[test]
fn insert_at_cursor() {
    let prompt = PromptWidget::default();
    prompt.insert_str("ac");
    let _ = dispatch(Action::MoveLeft, &prompt);
    let _ = dispatch(Action::InsertChar('b'), &prompt);
    assert_eq!(prompt.text(), "abc");
}

#[tokio::test]
async fn extra_slash_prompt_sends_template() {
    use cordis::Context;
    use cordis_spine::{slash, ExtraSlashKind, Slash, SlashEntry, SLASH};

    let root = Context::new();
    root.plugin(slash(), ()).unwrap().wait().await.unwrap();
    let slash = root.require::<Slash>(SLASH).unwrap();
    let _own = slash
        .register(SlashEntry {
            command: "standup".into(),
            description: "站会".into(),
            kind: ExtraSlashKind::Prompt,
            text: "写站会：{args}".into(),
            title: String::new(),
            send: true,
        })
        .unwrap();
    let prompt = PromptWidget::with_context(root.clone());
    prompt.insert_str("/stand");
    let inserted = dispatch(Action::SlashAccept, &prompt);
    assert!(inserted.is_empty(), "{inserted:?}");
    assert_eq!(prompt.text(), "/standup ");
    let effects = dispatch(Action::SendPrompt("/standup 今日".into()), &prompt);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SendPrompt {
                text,
                send_now: true
            }] if text == "写站会：今日"
        ),
        "{effects:?}"
    );
}

#[test]
fn queue_actions_dispatch() {
    let prompt = PromptWidget::default();
    assert!(matches!(
        dispatch(Action::PromoteQueued { id: None }, &prompt).as_slice(),
        [Effect::PromoteQueued { id: None }]
    ));
    let effects = dispatch(
        Action::EditQueued {
            id: Some("p1".into()),
        },
        &prompt,
    );
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::EditQueued { id: Some(id) }] if id == "p1"
        ),
        "{effects:?}"
    );
}

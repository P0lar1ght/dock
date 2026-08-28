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
fn slash_accept_uses_highlighted_row() {
    let prompt = PromptWidget::default();
    prompt.insert_str("/q");
    let effects = dispatch(Action::SlashAccept, &prompt);
    assert!(matches!(effects.as_slice(), [Effect::Quit]));
    assert!(prompt.text().is_empty());
}

#[test]
fn slash_plan_tasks_mcps_map() {
    let prompt = PromptWidget::default();
    let plan = dispatch(Action::SendPrompt("/plan".into()), &prompt);
    assert!(matches!(
        plan.as_slice(),
        [Effect::EnterPlan { description: None }]
    ));
    let plan_desc = dispatch(
        Action::SendPrompt("/plan refactor auth".into()),
        &prompt,
    );
    assert!(matches!(
        plan_desc.as_slice(),
        [Effect::EnterPlan { description: Some(d) }] if d == "refactor auth"
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/tasks".into()), &prompt).as_slice(),
        [Effect::ShowTasks]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/mcps".into()), &prompt).as_slice(),
        [Effect::ShowMcps]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/goal".into()), &prompt).as_slice(),
        [Effect::EnterGoal { objective: None }]
    ));
    assert!(matches!(
        dispatch(Action::SendPrompt("/goal ship the lsp tool".into()), &prompt).as_slice(),
        [Effect::EnterGoal { objective: Some(d) }] if d == "ship the lsp tool"
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

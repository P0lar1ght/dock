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
fn slash_history_maps_to_picker() {
    let prompt = PromptWidget::default();
    let effects = dispatch(Action::SendPrompt("/history".into()), &prompt);
    assert!(matches!(effects.as_slice(), [Effect::HistoryPicker]));
}

//! 改这一页设置的斜杠命令：`/model` `/protocol` `/effort` `/think` `/timestamps`。
//!
//! 空参数的 `/model` `/effort` 回 [`SlashOutcome::Menu`]，选择器由客户端自己画。

use cordis::{plugin, Context, Inject, Plugin};

use crate::host::settings::AppSettings;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{SETTINGS, SLASH};
use crate::ApiBackend;

pub fn settings_commands() -> Plugin {
    plugin("command-settings", Inject::from([SLASH]), |ctx, _: &()| {
        register_commands(
            ctx,
            vec![
                SlashCommand::host("model", "切换当前模型", slash_handler(model))
                    .aliases(&["m"])
                    .takes_args(true),
                SlashCommand::host(
                    "protocol",
                    "切换当前模型的推理协议",
                    slash_handler(protocol),
                )
                .aliases(&["proto", "wire"])
                .takes_args(true),
                SlashCommand::host(
                    "timestamps",
                    "开关滚动区时间戳",
                    slash_handler(|page, _| with_settings(page, timestamps)),
                ),
                SlashCommand::host(
                    "think",
                    "开关思考模式（推理过程）",
                    slash_handler(|page, _| with_settings(page, think)),
                )
                .aliases(&["thinking"]),
                SlashCommand::host("effort", "设置推理强度", slash_handler(effort))
                    .takes_args(true),
            ],
        )?;
        Ok(None)
    })
}

fn with_settings(page: &Context, f: impl FnOnce(&AppSettings) -> SlashOutcome) -> SlashOutcome {
    match page.get::<AppSettings>(SETTINGS) {
        Some(settings) => f(&settings),
        None => SlashOutcome::notice("设置", "设置服务未挂载。"),
    }
}

fn model(page: &Context, args: &str) -> SlashOutcome {
    if args.is_empty() {
        return SlashOutcome::Menu("model".into());
    }
    with_settings(page, |settings| {
        settings.set_model(args);
        SlashOutcome::Applied(format!("已切换 {args}"))
    })
}

/// 在当前模型 `api_backends` 声明过的协议之间切。空参数回一段可选项：为它新开一个
/// menu id 要扩 dock.1，不值得。
fn protocol(page: &Context, args: &str) -> SlashOutcome {
    with_settings(page, |settings| {
        let choices = settings.backend_choices();
        if args.is_empty() {
            let current = settings.backend();
            let body = choices
                .iter()
                .map(|b| {
                    let mark = if *b == current { " ·当前" } else { "" };
                    format!("/protocol {}{mark} — {}", b.name(), b.description())
                })
                .collect::<Vec<_>>()
                .join("\n");
            return SlashOutcome::notice("推理协议", body);
        }
        let Some(backend) = ApiBackend::from_name(args) else {
            return SlashOutcome::Applied(format!("未知协议 {args}"));
        };
        if !choices.contains(&backend) {
            return SlashOutcome::Applied(format!(
                "{} 未在该模型的 api_backends 里声明",
                backend.name()
            ));
        }
        settings.set_backend(backend);
        SlashOutcome::Applied(format!("协议 {}", backend.name()))
    })
}

fn effort(page: &Context, args: &str) -> SlashOutcome {
    if args.is_empty() {
        return SlashOutcome::Menu("reasoning".into());
    }
    with_settings(page, |settings| {
        settings.set_effort(args);
        SlashOutcome::Applied(format!("推理强度 {args}"))
    })
}

fn think(settings: &AppSettings) -> SlashOutcome {
    let on = settings.toggle_thinking();
    SlashOutcome::Applied(
        if on {
            "思考模式已开"
        } else {
            "思考模式已关"
        }
        .into(),
    )
}

fn timestamps(settings: &AppSettings) -> SlashOutcome {
    let on = !settings.timestamps();
    settings.set_timestamps(on);
    SlashOutcome::Applied(
        if on {
            "时间戳已开"
        } else {
            "时间戳已关"
        }
        .into(),
    )
}

use cordis::{plugin, plugin_async, Inject, Plugin};
use cordis_spine::{register_commands, Slash, SLASH};

use crate::names::{
    SESSION, SESSION_PORT, THEME, TUI_PAIRING, TUI_PROMPT, TUI_SCROLLBACK, TUI_SHORTCUTS,
    TUI_STATUS, TUI_WELCOME,
};
use crate::scrollback::Scrollback;
use crate::theme::Theme;
use crate::views::prompt::PromptWidget;
use crate::views::status::StatusLine;
use crate::views::welcome::Welcome;

use crate::app::event_loop;

pub fn theme() -> Plugin {
    plugin("theme", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(THEME, Theme::groknight())?))
    })
}

pub fn scrollback() -> Plugin {
    plugin(
        "tui.scrollback",
        Inject::from(["sessions", THEME]),
        |ctx, _: &()| {
            Ok(Some(
                ctx.provide(TUI_SCROLLBACK, Scrollback::new(ctx.clone()))?,
            ))
        },
    )
}

pub fn prompt() -> Plugin {
    plugin("tui.prompt", Inject::from([THEME]), |ctx, _: &()| {
        Ok(Some(ctx.provide(
            TUI_PROMPT,
            PromptWidget::with_context(ctx.clone()),
        )?))
    })
}

pub fn status_bar() -> Plugin {
    plugin(
        "tui.statusBar",
        Inject::from(["sessions", THEME, SESSION_PORT]),
        |ctx, _: &()| Ok(Some(ctx.provide(TUI_STATUS, StatusLine::new(ctx.clone()))?)),
    )
}

pub fn welcome() -> Plugin {
    plugin(
        "tui.welcome",
        Inject::from(["sessions", THEME]),
        |ctx, _: &()| Ok(Some(ctx.provide(TUI_WELCOME, Welcome::new(ctx.clone()))?)),
    )
}

pub fn shortcuts() -> Plugin {
    crate::seam::shortcuts::shortcuts()
}

pub fn pairing() -> Plugin {
    plugin("tui.pairing", Inject::from([THEME]), |ctx, _: &()| {
        Ok(Some(ctx.provide(
            TUI_PAIRING,
            crate::views::pairing::PairingUi::new(ctx.clone()),
        )?))
    })
}

/// 把终端自己的斜杠命令登记进 `"slash"`（见 [`crate::slash::terminal_commands`]）。
/// 随 TUI 一起注销：无头的 `dock serve` 不挂它，那边的命令表里也就没有这些。
pub fn commands() -> Plugin {
    plugin("tui.commands", Inject::from([SLASH]), |ctx, _: &()| {
        let slash = ctx.require::<Slash>(SLASH)?;
        register_commands(ctx, crate::slash::terminal_commands(&slash))?;
        Ok(None)
    })
}

/// 终端 UI 的全部视图件，按挂载顺序。组合根（`cordis-app`）逐个挂，再挂 [`tui`]；
/// 想换哪一颗就在组合根换，不用动这里。
pub fn views() -> Vec<Plugin> {
    vec![
        theme(),
        scrollback(),
        prompt(),
        status_bar(),
        welcome(),
        shortcuts(),
        pairing(),
        commands(),
    ]
}

/// Pager event loop. 只跑事件循环：视图件由组合根挂（见 [`views`]），这里
/// inject 它们——换掉任何一颗都不用碰循环。
///
/// **必须先挂 [`views`]**。inject 的依赖没到齐时插件只是一直等着、不报错：忘了挂
/// 视图件，`tui` 会停在等依赖的状态，终端上什么都不出现。
pub fn tui() -> Plugin {
    plugin_async(
        "tui",
        Inject::from([
            SESSION,
            SESSION_PORT,
            THEME,
            TUI_SCROLLBACK,
            TUI_PROMPT,
            TUI_STATUS,
            TUI_WELCOME,
            TUI_SHORTCUTS,
            TUI_PAIRING,
        ]),
        |ctx, _: &()| async move {
            event_loop::run(ctx)
                .await
                .map_err(|e| cordis::Error::message(e.to_string()))?;
            Ok(None)
        },
    )
}

use cordis::{plugin, plugin_async, Inject, Plugin};

use crate::names::{
    SESSION, SESSION_PORT, THEME, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_WELCOME,
};
use crate::prompt::PromptWidget;
use crate::scrollback::Scrollback;
use crate::status::StatusLine;
use crate::theme::Theme;
use crate::welcome::Welcome;

use crate::event_loop;

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
        Ok(Some(ctx.provide(TUI_PROMPT, PromptWidget::default())?))
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
    crate::shortcuts::shortcuts()
}

/// Pager event loop. Injects session + view plugins; swap this plugin to
/// change the UI without touching the loop.
pub fn tui() -> Plugin {
    plugin_async(
        "tui",
        Inject::from([SESSION, SESSION_PORT]),
        |ctx, _: &()| async move {
            ctx.plugin(theme(), ())?.wait().await?;
            ctx.plugin(scrollback(), ())?.wait().await?;
            ctx.plugin(prompt(), ())?.wait().await?;
            ctx.plugin(status_bar(), ())?.wait().await?;
            ctx.plugin(welcome(), ())?.wait().await?;
            ctx.plugin(shortcuts(), ())?.wait().await?;
            event_loop::run(ctx)
                .await
                .map_err(|e| cordis::Error::message(e.to_string()))?;
            Ok(None)
        },
    )
}

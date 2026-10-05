//! 人用的 `/compact`：把这一页交给会话 actor 压缩（参数是给摘要的额外要求）。

use cordis::{plugin, Inject, Plugin};

use crate::host::session_port::SessionRef;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{SESSION_PORT, SLASH};

pub fn compact_command() -> Plugin {
    plugin("command-compact", Inject::from([SLASH]), |ctx, _: &()| {
        register_commands(
            ctx,
            vec![SlashCommand::host(
                "compact",
                "压缩旧对话",
                slash_handler(|page, args| match page.get::<SessionRef>(SESSION_PORT) {
                    Some(port) => {
                        port.compact(args.to_string());
                        SlashOutcome::Applied("正在压缩上下文…".into())
                    }
                    None => SlashOutcome::notice("压缩", "session.port 未挂载。"),
                }),
            )
            .takes_args(true)],
        )?;
        Ok(None)
    })
}

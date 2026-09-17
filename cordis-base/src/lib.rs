//! Dock spine 的底座：wire 类型、`config.toml`、纯引擎。
//!
//! **这里没有插件。** 判据就是这一条——本 crate 不 `provide` 任何 named service、
//! 不认识 ctx 键（所以连 `names` 都不在这儿）、也不依赖内核 crate `cordis`。
//! 插件树上的东西一律留在 `cordis-spine`。
//!
//! 拆出来的理由不是"文件太多"，是**依赖方向**：这十个模块谁都不回头依赖 spine
//! （唯一那条 `types → session` 已经在前一个 commit 解掉），把它们钉成独立 crate
//! 之后，编译器会替我们守住这个方向——以前只能靠约定。
//!
//! | 模块 | 是什么 |
//! |---|---|
//! | [`types`] | `LogEvent` / `ToolCall` / `ToolResult` 与会话身份原语 |
//! | [`config`] | `config.toml` 的解析与模型目录 |
//! | [`usage`] | 每次调用与每个会话的 token / 费用账本 |
//! | [`chat_chunk`] [`stream_acc`] | 三条 wire 的流式分片与累积 |
//! | [`acp`] | ACP 权限选项种类 |
//! | [`cua`] | cua-driver 的本机发现、安装与授权 |
//! | [`grep`] | 进程内 ripgrep 引擎（工具插件在 spine） |
//! | [`tool_output`] | 工具输出的共享预算：语义分页 + 溢出落盘 |
//! | [`test_env`] | 进程级 env / cwd 的测试互斥 |

pub mod acp;
pub mod chat_chunk;
pub mod config;
pub mod cua;
pub mod grep;
pub mod stream_acc;
pub mod test_env;
pub mod tool_output;
pub mod types;
pub mod usage;

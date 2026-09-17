//! 采样：抽象 sampler、三条 HTTP wire、会话压缩。
//!
//! [`sampler`] 是 `"llm"` named service 与 [`Sampler`](sampler::Sampler) trait；
//! [`http`] 是 responses / chat_completions / messages 三条线的实现；
//! [`compact`] 用一次采样把旧对话换成摘要。

pub mod compact;
pub mod http;
pub mod sampler;

//! 压缩进度：正在压缩时走到哪一阶段、摘要已经吐了多少，压完前后各占多少。
//!
//! 只在内存里，不落盘。每次变化 [`crate::Sessions`] 发一条
//! [`crate::SESSION_COMPACTION`]（载荷 [`PageCompaction`]）：网关投影成 dock.1
//! `context/compacted`，TUI 每帧直接读 [`crate::Sessions::compaction`]。

use std::sync::Arc;
use std::time::{Duration, Instant};

/// 谁发起的这次压缩。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactTrigger {
    /// 上下文到了阈值，循环在下一次采样前自己压。
    Auto,
    /// `/compact` 或网关 `thread/context/compact`。
    Manual,
}

impl CompactTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// 压缩走到了哪一步。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactPhase {
    /// 压缩前把值得记住的东西写进记忆（记忆 flush 开着且到了门槛才有）。
    Memory,
    /// 让模型写摘要。
    Summary,
    /// 用摘要替换模型历史、落压缩段。
    Apply,
}

impl CompactPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Summary => "summary",
            Self::Apply => "apply",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Memory => "整理记忆",
            Self::Summary => "生成摘要",
            Self::Apply => "替换历史",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactStatus {
    Running,
    Completed,
    /// 带给用户看的原因（`压缩失败：摘要过短。` 这种）。
    Failed(String),
    Cancelled,
}

impl CompactStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed(_) => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// 一次压缩的进展。跑完之后留着，直到下一次压缩或换会话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactProgress {
    pub trigger: CompactTrigger,
    pub status: CompactStatus,
    pub phase: CompactPhase,
    /// 第几次尝试，从 1 起。
    pub attempt: u32,
    pub max_attempts: u32,
    /// 上一次尝试为什么没成（重试时给用户看）。
    pub retry_reason: Option<String>,
    /// 这次尝试摘要模型已经输出的 token（正文 + 推理，估算）。
    pub output_tokens: u64,
    /// 压缩前的上下文占用。
    pub before_tokens: u64,
    /// 压缩后的上下文占用，压成了才有。
    pub after_tokens: Option<u64>,
    pub started: Instant,
    /// 跑完时定格的耗时；还在跑时是 `None`，按 `started` 现算。
    pub finished: Option<Duration>,
}

impl CompactProgress {
    pub(crate) fn start(trigger: CompactTrigger, max_attempts: u32, before_tokens: u64) -> Self {
        Self {
            trigger,
            status: CompactStatus::Running,
            phase: CompactPhase::Summary,
            attempt: 1,
            max_attempts,
            retry_reason: None,
            output_tokens: 0,
            before_tokens,
            after_tokens: None,
            started: Instant::now(),
            finished: None,
        }
    }

    pub fn running(&self) -> bool {
        self.status == CompactStatus::Running
    }

    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(|| self.started.elapsed())
    }
}

/// [`crate::SESSION_COMPACTION`] 的载荷：哪一页（`main` / `main#N`）的压缩变了。
#[derive(Clone, Debug)]
pub struct PageCompaction {
    pub page: Arc<str>,
    pub progress: CompactProgress,
}

/// 摘要流式输出的 token 计数，口径同上下文估算（ASCII 4 个算 1，其余各算 1）。
#[derive(Default)]
pub(crate) struct OutputCounter {
    ascii: u64,
    other: u64,
}

impl OutputCounter {
    pub(crate) fn push(&mut self, text: &str) -> u64 {
        for c in text.chars() {
            if c.is_ascii() {
                self.ascii += 1;
            } else {
                self.other += 1;
            }
        }
        self.other + self.ascii.saturating_add(3) / 4
    }
}

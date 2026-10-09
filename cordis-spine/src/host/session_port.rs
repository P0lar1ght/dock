//! 一页的提交口 `"session.port"`。`cordis-app` 的 `session_actor` provide，TUI、
//! 网关、定时任务驱动在发送点 live-lookup——不要把 Arc 关进长生命周期闭包。

use std::sync::Arc;

/// One prompt sitting in the actor queue (compact jobs are omitted).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedItem {
    pub id: String,
    pub text: String,
}

/// Queue a prompt / report whether a turn is in flight.
///
/// 一轮在跑时再发一条有三种意思：`submit(_, false)` 排队（这一轮结束后单独成
/// 一轮）；`steer` 插话（下一个步骤边界并进这一轮，不打断）；`submit(_, true)`
/// 停止并发送（停掉这一轮，马上发这一条）。
pub trait SessionPort: Send + Sync {
    fn submit(&self, text: String, send_now: bool);
    /// 插话。空闲（或正在压缩、没有采样可并）时退化为排到最前的一条消息。
    fn steer(&self, text: String, images: Vec<cordis_base::types::UserImage>);
    /// 把一条排队的消息改成插话。`None` = 最早那条。
    fn steer_queued(&self, id: Option<String>);
    fn working(&self) -> bool;
    fn cancel(&self);
    fn has_queued(&self) -> bool;
    fn compact(&self, context: String);
    fn queued_prompts(&self) -> Vec<QueuedItem>;
    fn promote(&self, id: Option<String>);
    fn take_queued(&self, id: Option<String>) -> Option<QueuedItem>;
}

/// Named `"session.port"` service. Clone is cheap; each call looks through to the actor.
#[derive(Clone)]
pub struct SessionRef(Arc<dyn SessionPort>);

impl SessionRef {
    pub fn new(port: Arc<dyn SessionPort>) -> Self {
        Self(port)
    }

    pub fn submit(&self, text: String, send_now: bool) {
        self.0.submit(text, send_now);
    }

    pub fn steer(&self, text: String, images: Vec<cordis_base::types::UserImage>) {
        self.0.steer(text, images);
    }

    pub fn steer_queued(&self, id: Option<String>) {
        self.0.steer_queued(id);
    }

    pub fn working(&self) -> bool {
        self.0.working()
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn has_queued(&self) -> bool {
        self.0.has_queued()
    }

    pub fn compact(&self, context: String) {
        self.0.compact(context);
    }

    pub fn queued_prompts(&self) -> Vec<QueuedItem> {
        self.0.queued_prompts()
    }

    pub fn promote(&self, id: Option<String>) {
        self.0.promote(id);
    }

    pub fn take_queued(&self, id: Option<String>) -> Option<QueuedItem> {
        self.0.take_queued(id)
    }
}

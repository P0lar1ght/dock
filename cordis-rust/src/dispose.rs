use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::error::Result;

pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub(crate) type Cleanup = Box<dyn FnOnce() -> BoxFuture<Result<()>> + Send>;

/// Cleanup handle. Calling [`Disposable::dispose`] twice is a no-op; a second
/// caller joins in-flight cleanup.
#[derive(Clone)]
pub struct Disposable {
    inner: Arc<DisposableInner>,
}

struct DisposableInner {
    label: String,
    children_meta: Mutex<Vec<EffectMeta>>,
    state: Mutex<DisposeState>,
}

enum DisposeState {
    Live {
        steps: Vec<CleanupStep>,
    },
    Running {
        done: tokio::sync::watch::Receiver<bool>,
    },
    Done,
}

pub(crate) enum CleanupStep {
    Sync(Box<dyn FnOnce() + Send>),
    Fn(Cleanup),
    Handle(Disposable),
}

/// Diagnostic node for currently registered labeled effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectMeta {
    pub label: String,
    pub children: Vec<EffectMeta>,
}

impl Disposable {
    pub fn from_fn<F>(f: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        Self::with_label(String::new(), vec![CleanupStep::Sync(Box::new(f))])
    }

    pub(crate) fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.inner, &b.inner)
    }

    pub fn from_async<F, Fut>(f: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::with_label(
            String::new(),
            vec![CleanupStep::Fn(Box::new(move || Box::pin(f())))],
        )
    }

    pub(crate) fn with_label(label: String, steps: Vec<CleanupStep>) -> Self {
        Self {
            inner: Arc::new(DisposableInner {
                label,
                children_meta: Mutex::new(Vec::new()),
                state: Mutex::new(DisposeState::Live { steps }),
            }),
        }
    }

    pub fn label(&self) -> &str {
        &self.inner.label
    }

    pub fn meta(&self) -> Option<EffectMeta> {
        if self.inner.label.is_empty() {
            return None;
        }
        Some(EffectMeta {
            label: self.inner.label.clone(),
            children: self.inner.children_meta.lock().unwrap().clone(),
        })
    }

    pub(crate) fn push_child_meta(&self, meta: EffectMeta) {
        self.inner.children_meta.lock().unwrap().push(meta);
    }

    pub(crate) fn take_live_steps(&self) -> Option<Vec<CleanupStep>> {
        let mut g = self.inner.state.lock().unwrap();
        match &mut *g {
            DisposeState::Live { steps } => Some(std::mem::take(steps)),
            _ => None,
        }
    }

    pub(crate) fn append_steps(&self, extra: Vec<CleanupStep>) {
        let mut g = self.inner.state.lock().unwrap();
        if let DisposeState::Live { steps } = &mut *g {
            steps.extend(extra);
        }
    }

    /// Run remaining **sync** cleanup now. Async steps are skipped (same as
    /// fiber error unwind). Further [`dispose`](Self::dispose) is a no-op.
    ///
    /// Dropping a [`Disposable`] does **not** run cleanup; callers that
    /// unregister live tools must call this (or [`dispose`](Self::dispose)).
    pub fn dispose_sync(&self) {
        let steps = {
            let mut g = self.inner.state.lock().unwrap();
            match &mut *g {
                DisposeState::Live { .. } => {
                    let DisposeState::Live { steps } =
                        std::mem::replace(&mut *g, DisposeState::Done)
                    else {
                        unreachable!()
                    };
                    steps
                }
                DisposeState::Done | DisposeState::Running { .. } => return,
            }
        };
        run_steps_sync(steps);
    }

    /// Run cleanup. Concurrent callers wait for the in-flight run.
    pub async fn dispose(&self) -> Result<()> {
        let taken = {
            let mut g = self.inner.state.lock().unwrap();
            match &mut *g {
                DisposeState::Done => return Ok(()),
                DisposeState::Running { done } => Err(done.clone()),
                DisposeState::Live { .. } => {
                    let (tx, rx) = tokio::sync::watch::channel(false);
                    let DisposeState::Live { steps } =
                        std::mem::replace(&mut *g, DisposeState::Running { done: rx })
                    else {
                        unreachable!()
                    };
                    Ok((steps, tx))
                }
            }
        };
        match taken {
            Err(mut rx) => {
                while !*rx.borrow() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                Ok(())
            }
            Ok((steps, tx)) => {
                let result = Box::pin(run_steps(steps)).await;
                let _ = tx.send(true);
                *self.inner.state.lock().unwrap() = DisposeState::Done;
                result
            }
        }
    }
}

pub(crate) fn run_steps_sync(mut steps: Vec<CleanupStep>) {
    steps.reverse();
    for step in steps {
        match step {
            CleanupStep::Sync(f) => f(),
            CleanupStep::Fn(_) | CleanupStep::Handle(_) => {}
        }
    }
}

async fn run_steps(mut steps: Vec<CleanupStep>) -> Result<()> {
    steps.reverse();
    for step in steps {
        match step {
            CleanupStep::Sync(f) => f(),
            CleanupStep::Fn(f) => f().await?,
            CleanupStep::Handle(d) => Box::pin(d.dispose()).await?,
        }
    }
    Ok(())
}

use std::sync::{Arc, Mutex};

/// Log severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

/// One log record.
#[derive(Clone, Debug)]
pub struct Message {
    pub sn: u64,
    pub name: String,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Clone)]
pub struct Logger {
    inner: Arc<Mutex<LoggerInner>>,
}

struct LoggerInner {
    sn: u64,
    buffer_size: usize,
    buffer: Vec<Message>,
    errors: Vec<String>,
}

impl Logger {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LoggerInner {
                sn: 0,
                buffer_size: 1000,
                buffer: Vec::new(),
                errors: Vec::new(),
            })),
        }
    }

    pub fn buffer_size(&self) -> usize {
        self.inner.lock().unwrap().buffer_size
    }

    pub fn set_buffer_size(&self, size: usize) {
        let mut g = self.inner.lock().unwrap();
        g.buffer_size = size;
        Self::trim(&mut g);
    }

    pub fn buffer(&self) -> Vec<Message> {
        self.inner.lock().unwrap().buffer.clone()
    }

    /// Errors recorded through [`Self::error`]. Useful in tests.
    pub fn errors(&self) -> Vec<String> {
        self.inner.lock().unwrap().errors.clone()
    }

    pub fn error(&self, text: impl Into<String>) {
        let text = text.into();
        {
            let mut g = self.inner.lock().unwrap();
            g.errors.push(text.clone());
        }
        self.push("root", LogLevel::Error, text);
    }

    pub fn warn(&self, text: impl Into<String>) {
        self.push("root", LogLevel::Warn, text.into());
    }

    pub fn info(&self, text: impl Into<String>) {
        self.push("root", LogLevel::Info, text.into());
    }

    pub fn debug(&self, text: impl Into<String>) {
        self.push("root", LogLevel::Debug, text.into());
    }

    pub(crate) fn named_error(&self, name: &str, text: impl Into<String>) {
        let text = text.into();
        {
            let mut g = self.inner.lock().unwrap();
            g.errors.push(text.clone());
        }
        self.push(name, LogLevel::Error, text);
    }

    fn push(&self, name: &str, level: LogLevel, text: String) {
        let mut g = self.inner.lock().unwrap();
        let sn = {
            g.sn += 1;
            g.sn
        };
        g.buffer.push(Message {
            sn,
            name: name.to_string(),
            level,
            text,
        });
        Self::trim(&mut g);
    }

    fn trim(g: &mut LoggerInner) {
        if g.buffer_size == 0 {
            g.buffer.clear();
            return;
        }
        let overflow = g.buffer.len().saturating_sub(g.buffer_size);
        if overflow > 0 {
            g.buffer.drain(0..overflow);
        }
    }
}

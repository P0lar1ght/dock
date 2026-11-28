//! WebSocket 的审计：握手之后来回的消息。
//!
//! 出口代理在转发的字节流里认帧（[`Decoder`]，RFC 6455；协商了 permessage-deflate 就跟着解压，
//! 转发的字节原样不动），浏览器从 CDP 拿现成的帧。两边都交给 [`Recorder`]：
//!
//! - 握手照常记一条 `net`（`scheme` 是 `ws` / `wss`、状态 101），带上 `ws.conn`；
//! - 之后的消息攒着，每 [`FLUSH`] 写一段 `kind = "ws"` 的记录（同一个 `conn`）：连接开着的时候
//!   界面也看得到；关了、断了（或者 Dock 退出）写最后一段（`done`）。
//! - 一条连接最多留 [`KEEP`] 条消息，每个方向留下的内容合计最多 `cap` 字节（和 HTTP 正文同一个
//!   上限 `body_cap_kb`）；超出的只计条数和字节。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use flate2::{Decompress, FlushDecompress, Status};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::audit::record::{now_ms, Captured, Flow};

/// 攒着的消息多久写一段。
pub const FLUSH: Duration = Duration::from_secs(2);
/// 留下的内容用完了以后，只有计数在变：隔这么久才写一段。
const HEARTBEAT: Duration = Duration::from_secs(30);
/// 一条连接最多留这么多条消息（之后只计数）。
pub const KEEP: usize = 2000;
/// 一条消息解压出来超过这么多就不再解：防压缩炸弹拖住代理。
const INFLATE_MAX: u64 = 64 * 1024 * 1024;
/// permessage-deflate 每条消息末尾省掉的同步块（RFC 7692 §7.2.2），解压前补回去。
const DEFLATE_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// 记录交给谁（和 [`crate::audit::proxy::Sink`] 同一个签名）。
pub type Sink = Arc<dyn Fn(Flow) + Send + Sync>;

/// [`Flow::ws`]：握手那条只有 `conn`；`kind = "ws"` 的一段还带 [`Part`]。
#[derive(Debug, Clone, Serialize)]
pub struct Ws {
    /// 这条连接的 id（握手和之后的每一段都带，按它串起来）。
    pub conn: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<Part>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Part {
    /// 第几段（从 1 数）。
    pub n: u64,
    /// 这一段记下的消息条数；内容是一个 JSON 数组，存成正文块（`messages.sha256`）。
    pub count: u64,
    pub messages: Captured,
    /// 到这一段为止，发出 / 收到各多少条消息（含 ping / pong / 关闭帧，含没留下来的）。
    pub sent: u64,
    pub received: u64,
    /// 到这一段为止没留下来的条数（超出 [`KEEP`]）。
    pub omitted: u64,
    /// 最后一段：连接关了或断了。
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<Close>,
}

/// 关闭帧：谁先发的、关闭码、原因。
#[derive(Debug, Clone, Serialize)]
pub struct Close {
    /// `client` / `server`。
    pub by: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// 方向：`Up` 是客户端发出去的，`Down` 是收到的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
}

impl Dir {
    fn name(self) -> &'static str {
        match self {
            Dir::Up => "up",
            Dir::Down => "down",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// 帧的操作码（只认 RFC 6455 定义了的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl Op {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Op::Text,
            2 => Op::Binary,
            8 => Op::Close,
            9 => Op::Ping,
            10 => Op::Pong,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Op::Text => "text",
            Op::Binary => "binary",
            Op::Close => "close",
            Op::Ping => "ping",
            Op::Pong => "pong",
        }
    }
}

/// 一条消息的内容：总长度和开头一段（最多 `cap`）。
#[derive(Debug, Default)]
pub struct Payload<'a> {
    pub size: u64,
    pub sample: &'a [u8],
    pub truncated: bool,
    /// 按 permessage-deflate 解压过。
    pub compressed: bool,
    /// 内容没法看的原因（扩展不认识、解压失败）。
    pub note: Option<&'a str>,
    /// 连上之后第几毫秒（浏览器按 CDP 自己的时间给）；不给就按收到的时候算。
    pub at: Option<u64>,
}

/// 新连接的 id：同一次运行里不重复，跨运行带着毫秒时间也撞不上。
pub fn conn_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("ws-{:x}-{}", now_ms(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// 一条连接的记录：收消息、按时写段。最后一份放掉时没 [`Recorder::finish`] 过也会补上最后一段。
pub struct Recorder {
    state: Mutex<State>,
    sink: Sink,
}

struct State {
    conn: String,
    tag: String,
    scheme: &'static str,
    host: String,
    port: u16,
    decrypted: bool,
    started_ms: u64,
    t0: Instant,
    /// 每个方向还能留多少内容。
    room: [usize; 2],
    buf: Vec<Value>,
    kept: usize,
    counts: [u64; 2],
    omitted: u64,
    /// 还没写出去的线上字节。
    wire: [u64; 2],
    /// 上一段之后计数变过（有消息或字节）。
    changed: bool,
    part: u64,
    last: Instant,
    close: Option<Close>,
    error: Option<String>,
    note: Option<String>,
    done: bool,
}

impl Recorder {
    /// 开始记一条连接（握手那条由调用方自己记，带上 [`Recorder::conn`]）。在 tokio 运行时里调
    /// 才会按时写段；不在就只在 [`Recorder::finish`] 时写一段。
    #[allow(clippy::too_many_arguments)] // 一条出网记录的全部字段
    pub fn start(
        sink: Sink,
        conn: String,
        tag: impl Into<String>,
        scheme: &'static str,
        host: impl Into<String>,
        port: u16,
        decrypted: bool,
        cap: usize,
    ) -> Arc<Self> {
        let rec = Arc::new(Self {
            state: Mutex::new(State {
                conn,
                tag: tag.into(),
                scheme,
                host: host.into(),
                port,
                decrypted,
                started_ms: now_ms(),
                t0: Instant::now(),
                room: [cap, cap],
                buf: Vec::new(),
                kept: 0,
                counts: [0, 0],
                omitted: 0,
                wire: [0, 0],
                changed: false,
                part: 0,
                last: Instant::now(),
                close: None,
                error: None,
                note: None,
                done: false,
            }),
            sink,
        });
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let weak: Weak<Self> = Arc::downgrade(&rec);
            rt.spawn(async move {
                let mut tick = tokio::time::interval(FLUSH);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    let Some(rec) = weak.upgrade() else { break };
                    if !rec.tick() {
                        break;
                    }
                }
            });
        }
        rec
    }

    pub fn conn(&self) -> String {
        self.state.lock().unwrap().conn.clone()
    }

    /// 线上的字节（帧头、掩码、压缩后的内容都算）。
    pub fn wire(&self, dir: Dir, n: u64) {
        let mut s = self.state.lock().unwrap();
        s.wire[dir.index()] += n;
        s.changed = true;
    }

    /// 一条完整的消息（分片已经拼好、压缩已经解开）。
    pub fn message(&self, dir: Dir, op: Op, payload: Payload<'_>) {
        let mut s = self.state.lock().unwrap();
        if s.done {
            return;
        }
        s.counts[dir.index()] += 1;
        s.changed = true;
        if op == Op::Close && s.close.is_none() {
            let (code, reason) = close_reason(payload.sample);
            s.close = Some(Close {
                by: if dir == Dir::Up { "client" } else { "server" },
                code,
                reason,
            });
        }
        if s.kept >= KEEP {
            s.omitted += 1;
            return;
        }
        s.kept += 1;
        let at = payload
            .at
            .unwrap_or_else(|| s.t0.elapsed().as_millis() as u64);
        let room = &mut s.room[dir.index()];
        let take = payload.sample.len().min(*room);
        *room -= take;
        let sample = &payload.sample[..take];
        let mut m = Map::new();
        m.insert("dir".into(), json!(dir.name()));
        m.insert("at".into(), json!(at));
        m.insert("op".into(), json!(op.name()));
        m.insert("size".into(), json!(payload.size));
        if op == Op::Close {
            let (code, reason) = close_reason(payload.sample);
            m.insert("code".into(), json!(code));
            m.insert("reason".into(), json!(reason));
        } else if !sample.is_empty() {
            match (
                op,
                utf8_prefix(sample, take < payload.sample.len() || payload.truncated),
            ) {
                (Op::Text, Some(text)) => m.insert("text".into(), json!(text)),
                _ => m.insert(
                    "base64".into(),
                    json!(base64::engine::general_purpose::STANDARD.encode(sample)),
                ),
            };
        }
        if payload.truncated || ((take as u64) < payload.size && payload.note.is_none()) {
            m.insert("truncated".into(), json!(true));
        }
        if payload.compressed {
            m.insert("compressed".into(), json!(true));
        }
        if let Some(note) = payload.note {
            m.insert("note".into(), json!(note));
        }
        s.buf.push(Value::Object(m));
    }

    /// 帧出错之类：记在下一段里（不算关闭）。
    pub fn error(&self, error: impl Into<String>) {
        let mut s = self.state.lock().unwrap();
        s.error = Some(error.into());
        s.changed = true;
    }

    /// 一句说明（例如帧认不出了、之后只计字节）。
    pub fn note(&self, note: impl Into<String>) {
        let mut s = self.state.lock().unwrap();
        s.note = Some(note.into());
        s.changed = true;
    }

    /// 连接结束：写最后一段。`error` 是连接断开的原因；双方已经交换过关闭帧时不算出错。
    pub fn finish(&self, error: Option<String>) {
        let mut s = self.state.lock().unwrap();
        if s.done {
            return;
        }
        if s.close.is_none() {
            if let Some(e) = error {
                s.error.get_or_insert(e);
            }
        }
        s.done = true;
        emit(&self.sink, &mut s);
    }

    /// 按时写段：攒了消息就写；只有计数在变时隔 [`HEARTBEAT`] 才写。连接结束了给 `false`。
    fn tick(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.done {
            return false;
        }
        if !s.buf.is_empty() || (s.changed && s.last.elapsed() >= HEARTBEAT) {
            emit(&self.sink, &mut s);
        }
        true
    }
}

/// 写一段。在锁里调 sink（它只往通道里放）：段的先后不会乱。
fn emit(sink: &Sink, s: &mut State) {
    s.part += 1;
    let batch = std::mem::take(&mut s.buf);
    let mut messages = Captured::default();
    if !batch.is_empty() {
        let raw = serde_json::to_vec(&batch).unwrap_or_default();
        messages.push(&raw, raw.len());
    }
    let flow = Flow {
        tag: s.tag.clone(),
        kind: "ws",
        scheme: s.scheme,
        host: s.host.clone(),
        port: s.port,
        method: None,
        path: None,
        status: None,
        request: None,
        response: None,
        bytes_up: std::mem::take(&mut s.wire[0]),
        bytes_down: std::mem::take(&mut s.wire[1]),
        started_ms: s.started_ms,
        duration_ms: s.t0.elapsed().as_millis() as u64,
        decrypted: s.decrypted,
        note: s.note.take(),
        error: s.error.take(),
        ws: Some(Ws {
            conn: s.conn.clone(),
            part: Some(Part {
                n: s.part,
                count: batch.len() as u64,
                messages,
                sent: s.counts[0],
                received: s.counts[1],
                omitted: s.omitted,
                done: s.done,
                close: s.close.clone(),
            }),
        }),
    };
    s.changed = false;
    s.last = Instant::now();
    sink(flow);
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let s = self.state.get_mut().unwrap();
        if !s.done {
            s.note
                .get_or_insert_with(|| "记录停在连接还开着的时候（标签页关了或 Dock 退出）".into());
            s.done = true;
            emit(&self.sink, s);
        }
    }
}

/// 关闭帧的内容：两字节关闭码 + UTF-8 原因（都可以没有）。
fn close_reason(payload: &[u8]) -> (Option<u16>, String) {
    if payload.len() < 2 {
        return (None, String::new());
    }
    (
        Some(u16::from_be_bytes([payload[0], payload[1]])),
        String::from_utf8_lossy(&payload[2..]).into_owned(),
    )
}

/// 是 UTF-8 就给文本。`cut` 时开头一段可能断在一个字的中间：断掉的那半个字不算错。
fn utf8_prefix(bytes: &[u8], cut: bool) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s),
        Err(e) if cut && e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

/// 浏览器（CDP）给的一帧：内容已经是完整的、解压过的。截到 `cap` 交给 [`Recorder::message`]。
/// `at` 是 CDP 给的时间折成的「连上之后第几毫秒」。
pub fn record_frame(
    rec: &Recorder,
    dir: Dir,
    opcode: u8,
    payload: &[u8],
    cap: usize,
    at: Option<u64>,
) {
    let Some(op) = Op::from_code(opcode) else {
        return;
    };
    rec.wire(dir, payload.len() as u64);
    let take = payload.len().min(cap);
    rec.message(
        dir,
        op,
        Payload {
            size: payload.len() as u64,
            sample: &payload[..take],
            truncated: take < payload.len(),
            at,
            ..Payload::default()
        },
    );
}

/// 代理这边一个方向的帧解析：喂进转发的原样字节，认出一条条消息交给 [`Recorder`]。
/// 认不出了（不是 WebSocket 帧、扩展不认识）就停下，之后只计字节——转发不受影响。
pub struct Decoder {
    dir: Dir,
    cap: usize,
    /// 协商了 permessage-deflate。
    deflate: bool,
    head: Vec<u8>,
    frame: Option<Frame>,
    msg: Option<Msg>,
    inflater: Option<Decompress>,
    /// 上一条压缩流以 BFINAL 结束了：下一条从新的流开始。
    fresh: bool,
    /// 解压出过错：之后的压缩消息解不开了（上下文丢了）。
    inflate_dead: bool,
    broken: bool,
}

struct Frame {
    fin: bool,
    op: u8,
    rsv1: bool,
    mask: Option<[u8; 4]>,
    len: u64,
    seen: u64,
    /// 控制帧（最多 125 字节）整个留着。
    control: Vec<u8>,
}

struct Msg {
    op: Op,
    compressed: bool,
    /// 扩展不认识（RSV 位、或没协商就压缩）：内容不看，只计大小。
    opaque: bool,
    size: u64,
    sample: Vec<u8>,
    truncated: bool,
    note: Option<&'static str>,
}

impl Msg {
    fn push(&mut self, bytes: &[u8], cap: usize) {
        self.size += bytes.len() as u64;
        let room = cap.saturating_sub(self.sample.len());
        if room < bytes.len() {
            self.truncated = true;
        }
        self.sample
            .extend_from_slice(&bytes[..room.min(bytes.len())]);
    }
}

impl Decoder {
    pub fn new(dir: Dir, cap: usize, deflate: bool) -> Self {
        Self {
            dir,
            cap,
            deflate,
            head: Vec::with_capacity(14),
            frame: None,
            msg: None,
            inflater: None,
            fresh: false,
            inflate_dead: false,
            broken: false,
        }
    }

    pub fn dir(&self) -> Dir {
        self.dir
    }

    pub fn feed(&mut self, mut data: &[u8], rec: &Recorder) {
        while !data.is_empty() && !self.broken {
            if self.frame.is_none() {
                let used = self.read_head(data, rec);
                data = &data[used..];
                continue;
            }
            let f = self.frame.as_mut().expect("上面刚判过");
            let n = (f.len - f.seen).min(data.len() as u64) as usize;
            let mut chunk = data[..n].to_vec();
            if let Some(m) = f.mask {
                for (i, b) in chunk.iter_mut().enumerate() {
                    *b ^= m[((f.seen + i as u64) % 4) as usize];
                }
            }
            f.seen += n as u64;
            let ended = f.seen == f.len;
            let control = f.op >= 8;
            if control {
                f.control.extend_from_slice(&chunk);
            }
            data = &data[n..];
            if !control {
                self.payload(&chunk);
            }
            if ended {
                self.end_frame(rec);
            }
        }
    }

    /// 攒帧头；齐了就开一帧。给回用掉了多少字节。
    fn read_head(&mut self, data: &[u8], rec: &Recorder) -> usize {
        let mut used = 0;
        while self.head.len() < 2 && used < data.len() {
            self.head.push(data[used]);
            used += 1;
        }
        if self.head.len() < 2 {
            return used;
        }
        let masked = self.head[1] & 0x80 != 0;
        let need =
            2 + match self.head[1] & 0x7f {
                126 => 2,
                127 => 8,
                _ => 0,
            } + if masked { 4 } else { 0 };
        let more = (need - self.head.len()).min(data.len() - used);
        self.head.extend_from_slice(&data[used..used + more]);
        used += more;
        if self.head.len() < need {
            return used;
        }
        let h = std::mem::take(&mut self.head);
        let (b0, b1) = (h[0], h[1]);
        let (len, at) = match b1 & 0x7f {
            126 => (u16::from_be_bytes([h[2], h[3]]) as u64, 4),
            127 => (u64::from_be_bytes(h[2..10].try_into().unwrap()), 10),
            n => (n as u64, 2),
        };
        let frame = Frame {
            fin: b0 & 0x80 != 0,
            op: b0 & 0x0f,
            rsv1: b0 & 0x40 != 0,
            mask: masked.then(|| [h[at], h[at + 1], h[at + 2], h[at + 3]]),
            len,
            seen: 0,
            control: Vec::new(),
        };
        if let Err(why) = self.start_frame(&frame, b0 & 0x30 != 0) {
            self.broken = true;
            let side = if self.dir == Dir::Up {
                "发出"
            } else {
                "收到"
            };
            rec.note(format!("{side}的帧认不出（{why}），之后只计字节"));
            return used;
        }
        self.frame = Some(frame);
        if len == 0 {
            self.end_frame(rec);
        }
        used
    }

    /// 校验帧头、开始一条消息。
    fn start_frame(&mut self, f: &Frame, rsv23: bool) -> Result<(), String> {
        if f.len > i64::MAX as u64 {
            return Err("长度不对".into());
        }
        if f.op >= 8 {
            if Op::from_code(f.op).is_none() {
                return Err(format!("操作码 {}", f.op));
            }
            if !f.fin || f.len > 125 {
                return Err("控制帧分片或太长".into());
            }
            return Ok(());
        }
        match (f.op, self.msg.is_some()) {
            (0, false) => return Err("续帧前面没有消息".into()),
            (0, true) => return Ok(()),
            (_, true) => return Err("上一条消息还没完就开了新的".into()),
            _ => {}
        }
        let op = Op::from_code(f.op).ok_or_else(|| format!("操作码 {}", f.op))?;
        let compressed = f.rsv1 && self.deflate && !rsv23;
        let opaque = rsv23 || (f.rsv1 && !self.deflate);
        let mut note = opaque.then_some("用了不认识的扩展，内容没记");
        if compressed {
            if self.inflate_dead {
                note = Some("压缩的上下文断了，内容解不开");
            } else if self.fresh {
                if let Some(d) = self.inflater.as_mut() {
                    d.reset(false);
                }
                self.fresh = false;
            }
        }
        self.msg = Some(Msg {
            op,
            compressed,
            opaque: opaque || (compressed && self.inflate_dead),
            size: 0,
            sample: Vec::new(),
            truncated: false,
            note,
        });
        Ok(())
    }

    /// 数据帧的一段内容（已去掉掩码）。
    fn payload(&mut self, chunk: &[u8]) {
        let Some(mut msg) = self.msg.take() else {
            return;
        };
        if msg.opaque {
            msg.size += chunk.len() as u64;
        } else if msg.compressed {
            self.inflate(chunk, &mut msg, false);
        } else {
            msg.push(chunk, self.cap);
        }
        self.msg = Some(msg);
    }

    fn inflate(&mut self, mut input: &[u8], msg: &mut Msg, tail: bool) {
        let d = self.inflater.get_or_insert_with(|| Decompress::new(false));
        let mut out = vec![0u8; 16 * 1024];
        loop {
            let (in0, out0) = (d.total_in(), d.total_out());
            let flush = if tail {
                FlushDecompress::Sync
            } else {
                FlushDecompress::None
            };
            let status = d.decompress(input, &mut out, flush);
            let used = (d.total_in() - in0) as usize;
            let made = (d.total_out() - out0) as usize;
            input = &input[used..];
            msg.push(&out[..made], self.cap);
            match status {
                Ok(Status::StreamEnd) => {
                    self.fresh = true;
                    return;
                }
                Ok(_) if msg.size > INFLATE_MAX => {
                    self.kill_inflate(msg, "解压出来太大，没再解");
                    return;
                }
                Ok(_) if used == 0 && made == 0 => return,
                Ok(_) if input.is_empty() && made < out.len() => return,
                Ok(_) => {}
                Err(_) => {
                    self.kill_inflate(msg, "解压失败，内容没记");
                    return;
                }
            }
        }
    }

    fn kill_inflate(&mut self, msg: &mut Msg, note: &'static str) {
        self.inflate_dead = true;
        msg.opaque = true;
        msg.note = Some(note);
    }

    fn end_frame(&mut self, rec: &Recorder) {
        let Some(f) = self.frame.take() else {
            return;
        };
        if f.op >= 8 {
            let op = Op::from_code(f.op).expect("start_frame 校验过");
            rec.message(
                self.dir,
                op,
                Payload {
                    size: f.control.len() as u64,
                    sample: &f.control,
                    ..Payload::default()
                },
            );
            return;
        }
        if !f.fin {
            return;
        }
        let Some(mut msg) = self.msg.take() else {
            return;
        };
        // 流在这条消息里以 BFINAL 结束了就不用补尾巴。
        if msg.compressed && !msg.opaque && !self.fresh {
            self.inflate(&DEFLATE_TAIL, &mut msg, true);
        }
        let sample = if msg.opaque { &[][..] } else { &msg.sample[..] };
        rec.message(
            self.dir,
            msg.op,
            Payload {
                size: msg.size,
                sample,
                truncated: msg.truncated,
                compressed: msg.compressed && !msg.opaque,
                note: msg.note,
                at: None,
            },
        );
    }
}

/// 读回：一条连接到目前为止的各段（`kind = "ws"` 的记录，按时间排好）汇总成界面要的样子。
/// `live` = 记它的那次运行还在写；不在写又没有最后一段的，是 Dock 退出时还开着（不算开着）。
pub fn summary<'a>(parts: impl IntoIterator<Item = &'a Value>, live: bool) -> Value {
    let (mut up, mut down) = (0u64, 0u64);
    let mut last: Option<&Value> = None;
    let mut close = Value::Null;
    let mut error = Value::Null;
    let mut notes: Vec<&str> = Vec::new();
    for p in parts {
        up += p["bytes_up"].as_u64().unwrap_or(0);
        down += p["bytes_down"].as_u64().unwrap_or(0);
        let part = &p["ws"]["part"];
        if !part["close"].is_null() {
            close = part["close"].clone();
        }
        if p["error"].is_string() {
            error = p["error"].clone();
        }
        if let Some(n) = p["note"].as_str() {
            if !notes.contains(&n) {
                notes.push(n);
            }
        }
        last = Some(p);
    }
    let part = last.map(|p| &p["ws"]["part"]);
    let done = part.is_some_and(|p| p["done"] == true);
    if !done && !live {
        notes.push("Dock 那次运行结束时还开着，没记到关闭");
    }
    let count = |k: &str| part.and_then(|p| p[k].as_u64()).unwrap_or(0);
    json!({
        "open": !done && live,
        "sent": count("sent"),
        "received": count("received"),
        "omitted": count("omitted"),
        "close": close,
        "error": error,
        "note": if notes.is_empty() { Value::Null } else { json!(notes.join("；")) },
        "bytesUp": up,
        "bytesDown": down,
        "durationMs": last.and_then(|p| p["duration_ms"].as_u64()),
    })
}

/// 同 [`summary`]，再带上每一段存下的消息（从 `bodies` 里读回来拼好）。
pub fn conversation(parts: &[&Value], bodies: &Path, live: bool) -> Value {
    let mut v = summary(parts.iter().copied(), live);
    let mut messages = Vec::new();
    let mut lost = 0u64;
    for p in parts {
        let m = &p["ws"]["part"]["messages"];
        let Some(sha) = m["sha256"].as_str() else {
            continue;
        };
        // 只认 64 位十六进制：日志被改过也不拿它去读 `bodies/` 以外的文件。
        let batch = crate::audit::packet::is_sha256(sha)
            .then(|| std::fs::read(bodies.join(sha)).ok())
            .flatten()
            .and_then(|raw| serde_json::from_slice::<Vec<Value>>(&raw).ok());
        match batch {
            Some(batch) => messages.extend(batch),
            None => lost += p["ws"]["part"]["count"].as_u64().unwrap_or(0),
        }
    }
    // 按时间排（稳定）：浏览器那边两个方向的帧是分开收的，同一时刻的先后可能颠倒，`at` 才准。
    messages.sort_by_key(|m| m["at"].as_u64().unwrap_or(0));
    v["messages"] = json!(messages);
    v["lost"] = json!(lost);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(b0: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
        let mut out = vec![b0];
        let bit = if mask.is_some() { 0x80 } else { 0 };
        match payload.len() {
            n if n < 126 => out.push(bit | n as u8),
            n if n <= 0xffff => {
                out.push(bit | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(bit | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        match mask {
            Some(m) => {
                out.extend_from_slice(&m);
                out.extend(payload.iter().enumerate().map(|(i, b)| b ^ m[i % 4]));
            }
            None => out.extend_from_slice(payload),
        }
        out
    }

    fn rig(cap: usize) -> (Arc<Recorder>, Arc<Mutex<Vec<Flow>>>) {
        let flows: Arc<Mutex<Vec<Flow>>> = Arc::default();
        let sink = flows.clone();
        let rec = Recorder::start(
            Arc::new(move |f| sink.lock().unwrap().push(f)),
            conn_id(),
            "job-1",
            "ws",
            "example.com",
            80,
            true,
            cap,
        );
        (rec, flows)
    }

    fn messages(flows: &[Flow]) -> Vec<Value> {
        flows
            .iter()
            .flat_map(|f| {
                let sample = &f
                    .ws
                    .as_ref()
                    .unwrap()
                    .part
                    .as_ref()
                    .unwrap()
                    .messages
                    .sample;
                if sample.is_empty() {
                    Vec::new()
                } else {
                    serde_json::from_slice::<Vec<Value>>(sample).unwrap()
                }
            })
            .collect()
    }

    /// 帧头断在任何一个字节、分片的消息中间夹一个 ping、16 / 64 位长度：拆成一个字节一个字节
    /// 喂进去，结果和整块喂一样。
    #[test]
    fn frames_split_anywhere_decode_the_same() {
        let mask = Some([1, 2, 3, 4]);
        let long = vec![b'x'; 300];
        let mut wire = Vec::new();
        wire.extend(frame(0x01, b"hel", mask)); // 文本，没完
        wire.extend(frame(0x89, b"p", mask)); // 夹在中间的 ping
        wire.extend(frame(0x80, b"lo", mask)); // 续帧，完
        wire.extend(frame(0x82, &long, mask));
        wire.extend(frame(0x81, b"", mask));
        let (whole, whole_flows) = rig(1024);
        Decoder::new(Dir::Up, 1024, false).feed(&wire, &whole);
        whole.finish(None);
        let (bytes, byte_flows) = rig(1024);
        let mut d = Decoder::new(Dir::Up, 1024, false);
        for b in &wire {
            d.feed(std::slice::from_ref(b), &bytes);
        }
        bytes.finish(None);
        let a = messages(&whole_flows.lock().unwrap());
        let b = messages(&byte_flows.lock().unwrap());
        let strip = |v: &[Value]| -> Vec<Value> {
            v.iter()
                .map(|m| {
                    let mut m = m.clone();
                    m.as_object_mut().unwrap().remove("at");
                    m
                })
                .collect()
        };
        assert_eq!(strip(&a), strip(&b));
        let ops: Vec<(&str, u64)> = a
            .iter()
            .map(|m| (m["op"].as_str().unwrap(), m["size"].as_u64().unwrap()))
            .collect();
        assert_eq!(
            ops,
            [("ping", 1), ("text", 5), ("binary", 300), ("text", 0)]
        );
        assert_eq!(a[1]["text"], "hello");
    }

    /// 带上下文的 permessage-deflate：后一条引用前一条的内容；BFINAL 结束的流之后从新流开始。
    #[test]
    fn deflate_keeps_its_context_and_restarts_after_a_final_block() {
        use flate2::{Compress, Compression, FlushCompress};
        let mut z = Compress::new(Compression::default(), false);
        let mut pack = |text: &[u8]| {
            let mut out = Vec::with_capacity(text.len() + 64);
            z.compress_vec(text, &mut out, FlushCompress::Sync).unwrap();
            out.truncate(out.len() - 4);
            out
        };
        let mut wire = Vec::new();
        wire.extend(frame(0xc1, &pack(b"abcabcabc websocket"), None));
        wire.extend(frame(0xc1, &pack(b"websocket abcabcabc"), None));
        // 第三条用全新的流、带 BFINAL（no_context_takeover 时允许）。
        let mut fresh = Compress::new(Compression::default(), false);
        let mut fin = Vec::with_capacity(64);
        fresh
            .compress_vec(b"fresh start", &mut fin, FlushCompress::Finish)
            .unwrap();
        wire.extend(frame(0xc1, &fin, None));
        let mut again = Compress::new(Compression::default(), false);
        let mut next = Vec::with_capacity(64);
        again
            .compress_vec(b"after final", &mut next, FlushCompress::Sync)
            .unwrap();
        next.truncate(next.len() - 4);
        wire.extend(frame(0xc1, &next, None));
        let (rec, flows) = rig(1024);
        Decoder::new(Dir::Down, 1024, true).feed(&wire, &rec);
        rec.finish(None);
        let texts: Vec<String> = messages(&flows.lock().unwrap())
            .iter()
            .map(|m| {
                assert_eq!(m["compressed"], true, "{m}");
                m["text"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(
            texts,
            [
                "abcabcabc websocket",
                "websocket abcabcabc",
                "fresh start",
                "after final"
            ]
        );
    }

    /// 协商了 `*_no_context_takeover`：对端每条消息换一个新的压缩流、也不以 BFINAL 结束。
    /// 不用按参数重置：每条消息都结束在块边界上（同步块），新流的回引只指向它自己的内容，
    /// 一路接着解的 inflater 照样解得对。用重复的内容逼出回引，确保真走了这条路。
    #[test]
    fn deflate_without_context_takeover_decodes_without_a_reset() {
        use flate2::{Compress, Compression, FlushCompress};
        let texts = [
            "hello hello hello hello",
            "world world world world",
            "hello world hello world",
        ];
        let mut wire = Vec::new();
        for t in texts {
            let mut z = Compress::new(Compression::default(), false);
            let mut out = Vec::with_capacity(128);
            z.compress_vec(t.as_bytes(), &mut out, FlushCompress::Sync)
                .unwrap();
            assert!(out.ends_with(&DEFLATE_TAIL));
            out.truncate(out.len() - 4);
            assert!(out.len() < t.len(), "内容要压得动（有回引）");
            wire.extend(frame(0xc1, &out, None));
        }
        let (rec, flows) = rig(1024);
        Decoder::new(Dir::Down, 1024, true).feed(&wire, &rec);
        rec.finish(None);
        let got: Vec<String> = messages(&flows.lock().unwrap())
            .iter()
            .map(|m| m["text"].as_str().unwrap_or("<没解开>").to_string())
            .collect();
        assert_eq!(got, texts);
    }

    /// 内容按方向合计截到 `cap`；超过 [`KEEP`] 条只计数。
    #[test]
    fn content_and_message_count_are_capped() {
        let (rec, flows) = rig(8);
        let mut d = Decoder::new(Dir::Down, 8, false);
        d.feed(&frame(0x81, b"12345", None), &rec);
        d.feed(&frame(0x81, b"67890", None), &rec);
        for _ in 0..KEEP {
            d.feed(&frame(0x8a, b"", None), &rec);
        }
        rec.finish(None);
        let flows = flows.lock().unwrap();
        let msgs = messages(&flows);
        assert_eq!(msgs.len(), KEEP);
        assert_eq!(msgs[0]["text"], "12345");
        assert_eq!(
            (msgs[1]["text"].as_str(), msgs[1]["truncated"].as_bool()),
            (Some("678"), Some(true))
        );
        let last = flows
            .last()
            .unwrap()
            .ws
            .as_ref()
            .unwrap()
            .part
            .clone()
            .unwrap();
        assert_eq!(
            (last.received, last.omitted, last.done),
            (KEEP as u64 + 2, 2, true)
        );
    }

    /// 不是 WebSocket 帧了（保留的操作码）：停下、说明一句，之后的字节照常只计数，不当消息。
    #[test]
    fn garbage_stops_the_decoder_with_a_note() {
        let (rec, flows) = rig(64);
        let mut d = Decoder::new(Dir::Down, 64, false);
        d.feed(&frame(0x81, b"ok", None), &rec);
        d.feed(&[0x83, 0x00], &rec);
        d.feed(&frame(0x81, b"after", None), &rec);
        rec.finish(None);
        let flows = flows.lock().unwrap();
        let msgs = messages(&flows);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        let note = flows.last().unwrap().note.clone().unwrap();
        assert!(note.contains("认不出"), "{note}");
    }

    /// 关闭帧之后的断开不算出错；没关闭就断了记下原因。没 finish 就放掉也补最后一段。
    #[test]
    fn close_frames_decide_whether_a_disconnect_is_an_error() {
        let (rec, flows) = rig(64);
        let mut d = Decoder::new(Dir::Up, 64, false);
        let mut close = 1001u16.to_be_bytes().to_vec();
        close.extend_from_slice("走了".as_bytes());
        d.feed(&frame(0x88, &close, Some([9, 9, 9, 9])), &rec);
        rec.finish(Some("connection reset".into()));
        let f = flows.lock().unwrap().last().cloned().unwrap();
        assert_eq!(f.error, None);
        let close = f.ws.unwrap().part.unwrap().close.unwrap();
        assert_eq!(
            (close.by, close.code, close.reason.as_str()),
            ("client", Some(1001), "走了")
        );

        let (rec, flows) = rig(64);
        rec.finish(Some("connection reset".into()));
        assert_eq!(
            flows.lock().unwrap().last().unwrap().error.as_deref(),
            Some("connection reset")
        );

        let (rec, flows) = rig(64);
        rec.wire(Dir::Up, 3);
        drop(rec);
        let f = flows.lock().unwrap().last().cloned().unwrap();
        assert!(f.ws.unwrap().part.unwrap().done);
        assert_eq!(f.bytes_up, 3);
        assert!(f.note.unwrap().contains("还开着"));
    }

    /// 开着的连接按 [`FLUSH`] 写段，不等关闭。
    #[tokio::test]
    async fn an_open_connection_is_written_out_in_parts() {
        let (rec, flows) = rig(64);
        rec.message(
            Dir::Up,
            Op::Text,
            Payload {
                size: 2,
                sample: b"hi",
                ..Payload::default()
            },
        );
        tokio::time::sleep(FLUSH + Duration::from_millis(300)).await;
        {
            let flows = flows.lock().unwrap();
            assert_eq!(flows.len(), 1, "{flows:?}");
            let part = flows[0].ws.as_ref().unwrap().part.clone().unwrap();
            assert_eq!((part.n, part.count, part.sent, part.done), (1, 1, 1, false));
        }
        // 没有新消息：不再写（心跳之前）。
        tokio::time::sleep(FLUSH).await;
        assert_eq!(flows.lock().unwrap().len(), 1);
        rec.finish(None);
        assert_eq!(flows.lock().unwrap().len(), 2);
    }
}

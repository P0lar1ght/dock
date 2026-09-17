//! 会话名册：`$DOCK_HOME/sessions/` 下**全部 cwd** 的会话抬头。
//!
//! `Sessions::archived()`（走 [`session_persist::load_cwd`]）只看当前工作目录那
//! 一个子目录，而且会把每个会话的 `chat_history.jsonl` 整份解出来——它是给
//! `/resume` 用的，恢复谁就要谁的完整 transcript。名册要的是另一种东西：全机器
//! 的会话列表，每条只要标题、cwd、时间和一行摘要。
//!
//! 对应 Grok pager 的 `app/roster.rs`（dashboard 的行来源之一）。
//!
//! **不缓存进长生命周期闭包**：磁盘随时被别的 dock 进程改，和 `config` 目录同
//! 一条规矩。这里只压一层很短的 TTL 备忘，挡住"每帧重扫"这种误用。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cordis::{plugin, Inject, Plugin};

use crate::names::ROSTER;
use crate::session_persist::{self, RosterEntry};

/// 两次磁盘扫描之间的最短间隔。
///
/// 名册是给一个列表视图用的，视图每帧都会问一次；扫全部 cwd 要开几十次
/// `read_dir` 加同样多次小文件读，30fps 下不设这道闸就是每秒几千次系统调用。
/// 取 2s：人眼看一个列表的更新频率，远低于它。
const MEMO_TTL: Duration = Duration::from_secs(2);

struct Memo {
    at: Instant,
    rows: Vec<RosterEntry>,
}

/// 名册服务。`ctx.get::<Roster>(ROSTER)` live-lookup，不要把 `Arc` 关进闭包。
pub struct Roster {
    memo: Mutex<Option<Memo>>,
    /// 最近读过的那一份 transcript（id → events）。见 [`Roster::transcript`]。
    transcript_memo: Mutex<Option<(String, Vec<cordis_base::types::LogEvent>)>>,
}

impl Default for Roster {
    fn default() -> Self {
        Self::new()
    }
}

impl Roster {
    pub fn new() -> Self {
        Self {
            memo: Mutex::new(None),
            transcript_memo: Mutex::new(None),
        }
    }

    /// 全部 cwd 的会话，按更新时间新到旧。[`MEMO_TTL`] 内复用上一次的结果。
    pub fn list(&self) -> Vec<RosterEntry> {
        let mut memo = self.memo.lock().unwrap();
        if let Some(cached) = memo.as_ref() {
            if cached.at.elapsed() < MEMO_TTL {
                return cached.rows.clone();
            }
        }
        let rows = session_persist::load_roster();
        *memo = Some(Memo {
            at: Instant::now(),
            rows: rows.clone(),
        });
        rows
    }

    /// 丢掉备忘，下一次 [`list`](Self::list) 一定重扫。删除 / 新建会话之后调。
    pub fn invalidate(&self) {
        *self.memo.lock().unwrap() = None;
        *self.transcript_memo.lock().unwrap() = None;
    }

    /// 选中会话的完整 transcript，给面板 peek 用。
    ///
    /// **只缓存最近一个**：peek 一次只显示一个会话，所以同一个选中项的每帧重绘
    /// 都命中缓存；上下换选中项才重读一次，那是人手速度，一次文件读扛得住。
    /// 缓存不设 TTL——历史会话的 transcript 不会再变（还在写的那个是活的分页，
    /// 走的是内存里的 `Sessions`，不到这里来）。
    pub fn transcript(&self, id: &str, cwd: &std::path::Path) -> Vec<cordis_base::types::LogEvent> {
        {
            let cached = self.transcript_memo.lock().unwrap();
            if let Some((cached_id, events)) = cached.as_ref() {
                if cached_id == id {
                    return events.clone();
                }
            }
        }
        let events = session_persist::load_session(id, cwd)
            .map(|s| s.events)
            .unwrap_or_default();
        *self.transcript_memo.lock().unwrap() = Some((id.to_string(), events.clone()));
        events
    }

    /// 从磁盘删掉一个会话，并让备忘失效。
    ///
    /// 要 `cwd` 是因为会话按 cwd 分目录存；名册项自带 cwd，调用点直接把那个传进来。
    /// 走这条而不是让 TUI 直接调 `session_persist::remove`：删完必须 invalidate，
    /// 两步绑在一起才不会出现「删了但列表还在」。
    pub fn remove(&self, id: &str, cwd: &std::path::Path) -> std::io::Result<()> {
        let result = session_persist::remove(id, cwd);
        self.invalidate();
        result
    }
}

pub fn roster() -> Plugin {
    plugin("roster", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(ROSTER, Roster::new())?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ArchivedSession;
    use cordis_base::types::LogEvent;
    use std::path::Path;
    use std::time::SystemTime;

    fn archive(id: &str, title: &str, events: Vec<LogEvent>) -> ArchivedSession {
        let now = SystemTime::now();
        ArchivedSession {
            id: id.into(),
            title: title.into(),
            times: vec![now; events.len()],
            events,
            compact_prefix: None,
            compact_from: 0,
        }
    }

    /// 名册跨 cwd：两个不同工作目录下的会话都要列出来。
    /// `load_cwd` 只看当前目录那一个，这正是名册存在的理由。
    #[test]
    fn roster_spans_every_cwd() {
        let _env = cordis_base::test_env::scoped().home();
        session_persist::save(
            &archive("aaa", "第一个", vec![LogEvent::User("你好".into())]),
            Path::new("/tmp/project-one"),
        )
        .unwrap();
        session_persist::save(
            &archive("bbb", "第二个", vec![LogEvent::User("hi".into())]),
            Path::new("/tmp/project-two"),
        )
        .unwrap();

        let rows = Roster::new().list();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"aaa"), "{ids:?}");
        assert!(ids.contains(&"bbb"), "{ids:?}");

        let one = rows.iter().find(|r| r.id == "aaa").unwrap();
        assert_eq!(one.title, "第一个");
        assert_eq!(
            one.cwd,
            Path::new("/tmp/project-one"),
            "cwd 要从 meta.json 读回原样"
        );
    }

    /// cwd 只能从 `meta.json` 读：`encode_cwd_dirname` 把 `/` 和非字母数字都压成
    /// `-` 再折叠，从目录名反解不回来。这条用一个会被压缩的路径钉住。
    #[test]
    fn cwd_survives_the_lossy_directory_key() {
        let _env = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/a--b/c d/é");
        session_persist::save(
            &archive("ccc", "标题", vec![LogEvent::User("x".into())]),
            cwd,
        )
        .unwrap();

        let rows = Roster::new().list();
        let row = rows.iter().find(|r| r.id == "ccc").expect("在名册里");
        assert_eq!(row.cwd, cwd);
    }

    /// 摘要取**末条**事件，且压成一行——名册每条只占一行，换行会撑破它。
    #[test]
    fn summary_is_the_last_event_on_one_line() {
        let _env = cordis_base::test_env::scoped().home();
        session_persist::save(
            &archive(
                "ddd",
                "标题",
                vec![
                    LogEvent::User("第一句".into()),
                    LogEvent::User("最后\n一句  带换行".into()),
                ],
            ),
            Path::new("/tmp/summary-probe"),
        )
        .unwrap();

        let rows = Roster::new().list();
        let row = rows.iter().find(|r| r.id == "ddd").unwrap();
        assert_eq!(row.summary, "最后 一句 带换行");
    }

    /// 排序按更新时间新到旧。
    ///
    /// `meta.updated_unix` 是**秒**粒度，同一次测试里连着 save 出来的三个会话
    /// 时间戳会撞在一起，靠 sleep 排既慢又不稳。直接把 meta 重写成指定时间。
    #[test]
    fn newest_first() {
        let _env = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/order-probe");
        for (id, updated) in [("old", 1_000u64), ("new", 3_000), ("mid", 2_000)] {
            session_persist::save(&archive(id, id, vec![LogEvent::User(id.into())]), cwd).unwrap();
            let meta = session_persist::sessions_cwd_dir(cwd)
                .join(id)
                .join("meta.json");
            std::fs::write(
                &meta,
                format!(
                    r#"{{"id":"{id}","title":"{id}","cwd":"{}","updated_unix":{updated}}}"#,
                    cwd.display()
                ),
            )
            .unwrap();
        }
        let ids: Vec<String> = Roster::new().list().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["new", "mid", "old"], "{ids:?}");
    }

    /// 尾部扫描：transcript 大于 `TAIL_SCAN_BYTES` 时只读尾巴，而从中间切进来的
    /// 那半行必须被跳过。这条是 `last_history_line` 里唯一不平凡的分支——前面
    /// 几条用的都是小文件，`seek` 到 0，根本走不到。
    #[test]
    fn summary_reads_the_tail_of_a_large_transcript() {
        let _env = cordis_base::test_env::scoped().home();
        let filler = "x".repeat(4_000);
        let mut events: Vec<LogEvent> = (0..40)
            .map(|i| LogEvent::User(format!("{i}-{filler}")))
            .collect();
        events.push(LogEvent::User("最后一条".into()));
        let cwd = Path::new("/tmp/big-transcript");
        session_persist::save(&archive("big", "大会话", events), cwd).unwrap();

        let history = session_persist::sessions_cwd_dir(cwd)
            .join("big")
            .join("chat_history.jsonl");
        assert!(
            std::fs::metadata(&history).unwrap().len() > 64 * 1024,
            "这条用例要的就是超过尾扫窗口的文件"
        );

        let rows = Roster::new().list();
        let row = rows.iter().find(|r| r.id == "big").unwrap();
        assert_eq!(row.summary, "最后一条");
    }

    /// 按 id 取一份完整 transcript，且**只重读换了选中项的那一次**。
    /// 面板 peek 每帧都问，不缓存就是每帧一次文件读加一次整份反序列化。
    #[test]
    fn transcript_loads_one_session_and_caches_it() {
        let _env = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/peek-probe");
        session_persist::save(
            &archive(
                "s1",
                "会话一",
                vec![
                    LogEvent::User("第一句".into()),
                    LogEvent::User("第二句".into()),
                ],
            ),
            cwd,
        )
        .unwrap();
        session_persist::save(
            &archive("s2", "会话二", vec![LogEvent::User("另一个".into())]),
            cwd,
        )
        .unwrap();

        let roster = Roster::new();
        let one = roster.transcript("s1", cwd);
        assert_eq!(one.len(), 2, "要的是整份 transcript，不是一行摘要");
        assert!(matches!(&one[1], LogEvent::User(t) if t == "第二句"));

        // 磁盘上删掉，缓存里还在 —— 证明第二次没重读。
        session_persist::remove("s1", cwd).unwrap();
        assert_eq!(
            roster.transcript("s1", cwd).len(),
            2,
            "同一个 id 该命中缓存"
        );

        // 换一个 id 就重读。
        assert_eq!(roster.transcript("s2", cwd).len(), 1);
        // 换回来时 s1 已经被删了，读不到 —— 缓存只留最近一个。
        assert!(roster.transcript("s1", cwd).is_empty());
    }

    /// 读不到的 id 给空，不 panic（会话可能刚被别的进程删掉）。
    #[test]
    fn a_missing_transcript_is_empty_not_a_panic() {
        let _env = cordis_base::test_env::scoped().home();
        assert!(Roster::new()
            .transcript("nope", Path::new("/tmp/nowhere"))
            .is_empty());
    }

    /// 没有 transcript 的目录不是会话：`save` 对空 events 直接返回，这种目录只
    /// 会是写了一半或被手动动过的残留，列出来只是噪音。
    #[test]
    fn a_directory_without_a_transcript_is_not_a_session() {
        let _env = cordis_base::test_env::scoped().home();
        let stray = cordis_base::config::dock_home()
            .join("sessions")
            .join("hand-made")
            .join("no-history");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(
            stray.join("meta.json"),
            r#"{"id":"ghost","title":"幽灵","cwd":"/tmp"}"#,
        )
        .unwrap();

        let rows = Roster::new().list();
        assert!(
            !rows.iter().any(|r| r.id == "ghost"),
            "{:?}",
            rows.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
    }

    /// TTL 内不重扫：列表视图每帧都会问，不挡住就是每秒几千次 read_dir。
    #[test]
    fn list_is_memoized_within_the_ttl() {
        let _env = cordis_base::test_env::scoped().home();
        let roster = Roster::new();
        assert!(roster.list().is_empty());

        session_persist::save(
            &archive("late", "后来的", vec![LogEvent::User("x".into())]),
            Path::new("/tmp/memo-probe"),
        )
        .unwrap();
        assert!(roster.list().is_empty(), "TTL 内应当仍是上一次的结果");

        roster.invalidate();
        assert_eq!(roster.list().len(), 1, "invalidate 之后必须重扫");
    }
}

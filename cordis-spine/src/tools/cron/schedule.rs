//! 什么时候触发：固定间隔、cron 表达式（本机时区）、只跑一次、条件命令（定时跑一条
//! 命令，结果满足条件才触发）、文件变化、只手动。算下一次的时刻，以及给人（中文）和
//! 给模型（英文）的描述。

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDateTime, NaiveTime, TimeZone};
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

use super::CronError;
use crate::tools::sched::{interval_to_human, parse_interval};

pub(super) const DAY_MS: u64 = 86_400_000;

/// 条件命令最短多久查一次（同固定间隔的下限）。
pub const MIN_CHECK_EVERY: Duration = Duration::from_secs(60);
/// 条件命令默认多久查一次。
pub const DEFAULT_CHECK_EVERY: Duration = Duration::from_secs(300);
/// 文件变化默认等多久没有新改动才触发（一次保存常常连着写好几下）。
pub const DEFAULT_FILES_DEBOUNCE: Duration = Duration::from_secs(10);

/// 条件命令的结果什么时候算「满足」。除了 `Changed`，都是**从不满足变成满足**才触发
/// 一次——一直满足不会每查一次就叫一次。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WatchWhen {
    /// 输出（stdout + stderr）和上一次不一样。第一次只记基线、不触发。
    Changed,
    /// 退出码是 0。
    Succeeds,
    /// 退出码不是 0。
    Fails,
    /// 输出里能匹配到这个正则。
    Matches { pattern: String },
}

impl WatchWhen {
    fn human(&self) -> String {
        match self {
            Self::Changed => "output changes".into(),
            Self::Succeeds => "succeeds (exit 0)".into(),
            Self::Fails => "fails (non-zero exit)".into(),
            Self::Matches { pattern } => format!("output matches /{pattern}/"),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Changed => "结果变了就触发".into(),
            Self::Succeeds => "成功时触发".into(),
            Self::Fails => "失败时触发".into(),
            Self::Matches { pattern } => format!("输出匹配 /{pattern}/ 时触发"),
        }
    }
}

/// 条件命令：每隔 `every` 在任务的工作目录里跑一次 `command`（`bash -lc`），按 `when` 判断。
/// 只跑命令不花 token，满足条件才把提问送进会话（带上命令的输出）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchSpec {
    pub command: String,
    pub every: Duration,
    pub when: WatchWhen,
}

/// 文件变化：这些路径（相对任务的工作目录；可以是目录、文件或 glob，`~/` 开头算家目录）
/// 下有改动，等 `debounce` 没有新改动后触发一次，带上改了哪些文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilesSpec {
    pub paths: Vec<String>,
    pub debounce: Duration,
}

/// 触发方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CronSchedule {
    /// 固定间隔。相位从建任务那一刻起算，触发晚了也不漂。
    Every(Duration),
    /// 5 段 cron 表达式（分 时 日 月 周），按本机时区。用 [`CronSchedule::cron`] 建，会先校验。
    Cron(String),
    /// 只跑一次，墙钟毫秒。
    At(u64),
    /// 条件命令。用 [`CronSchedule::watch`] 建。
    Watch(WatchSpec),
    /// 文件变化。用 [`CronSchedule::files`] 建。
    Files(FilesSpec),
    /// 不自己触发：只在手动运行时跑（桌面端「立即运行」、`dock schedule run`、网关 `schedule/run`）。
    Manual,
}

impl CronSchedule {
    /// 校验过的 cron 表达式：5 段、以后还会触发。
    pub fn cron(expr: &str) -> Result<Self, CronError> {
        let expr = expr.trim();
        parse_cron(expr)?;
        if cron_next_after(expr, wall_ms()).is_none() {
            return Err(CronError::InvalidSchedule(format!(
                "cron 表达式 {expr:?} 以后不会再触发"
            )));
        }
        Ok(Self::Cron(expr.to_string()))
    }

    /// 校验过的条件命令：命令非空、间隔不短于 [`MIN_CHECK_EVERY`]、正则写得对。
    pub fn watch(command: &str, every: Duration, when: WatchWhen) -> Result<Self, CronError> {
        let command = command.trim();
        if command.is_empty() {
            return Err(CronError::InvalidSchedule("条件命令不能为空".into()));
        }
        if every < MIN_CHECK_EVERY {
            return Err(CronError::InvalidSchedule(format!(
                "条件命令最快每分钟查一次（收到 {} 秒）",
                every.as_secs()
            )));
        }
        if let WatchWhen::Matches { pattern } = &when {
            regex::Regex::new(pattern).map_err(|e| {
                CronError::InvalidSchedule(format!("正则 /{pattern}/ 写得不对：{e}"))
            })?;
        }
        Ok(Self::Watch(WatchSpec {
            command: command.to_string(),
            every,
            when,
        }))
    }

    /// 校验过的文件变化：至少一个非空路径，防抖至少 1 秒。
    pub fn files(paths: Vec<String>, debounce: Duration) -> Result<Self, CronError> {
        let paths: Vec<String> = paths
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if paths.is_empty() {
            return Err(CronError::InvalidSchedule("要监听的路径不能为空".into()));
        }
        Ok(Self::Files(FilesSpec {
            paths,
            debounce: debounce.max(Duration::from_secs(1)),
        }))
    }

    /// 会跑不止一次（一次性的不算）。
    pub fn is_recurring(&self) -> bool {
        !matches!(self, Self::At(_))
    }

    /// 网关与落盘用的名字：`every` / `cron` / `at` / `watch` / `files` / `manual`。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Every(_) => "every",
            Self::Cron(_) => "cron",
            Self::At(_) => "at",
            Self::Watch(_) => "watch",
            Self::Files(_) => "files",
            Self::Manual => "manual",
        }
    }

    /// 两次触发之间大概隔多久（cron 取接下来几次里最短的一段；条件命令取检查间隔）。
    /// 一次性、文件变化、手动的没有。用来判断「比一天还勤」——这类盯梢循环默认 7 天过期。
    pub fn period_hint(&self, now_ms: u64) -> Option<Duration> {
        match self {
            Self::Every(every) => Some(*every),
            Self::Cron(expr) => {
                let mut at = cron_next_after(expr, now_ms)?;
                let mut shortest: Option<u64> = None;
                for _ in 0..6 {
                    let next = cron_next_after(expr, at)?;
                    let gap = next.saturating_sub(at);
                    shortest = Some(shortest.map_or(gap, |s| s.min(gap)));
                    at = next;
                }
                shortest.map(Duration::from_millis)
            }
            Self::Watch(spec) => Some(spec.every),
            Self::At(_) | Self::Files(_) | Self::Manual => None,
        }
    }

    /// 给模型看的（英文，进 system-reminder）。
    pub fn human(&self) -> String {
        match self {
            Self::Every(every) => interval_to_human(every.as_secs()),
            Self::Cron(expr) => match parse_cron(expr) {
                Ok(cron) => format!("cron `{expr}` in local time ({})", cron.describe()),
                Err(_) => format!("cron `{expr}` in local time"),
            },
            Self::At(at) => format!("once, at {} local time", local_stamp(*at)),
            Self::Watch(spec) => format!(
                "when the check command `{}` {} (checked {})",
                spec.command,
                spec.when.human(),
                interval_to_human(spec.every.as_secs())
            ),
            Self::Files(spec) => format!("when files change under {}", spec.paths.join(", ")),
            Self::Manual => "only when started by hand".into(),
        }
    }

    /// 给人看的（中文）：「每 30 分钟」「工作日 09:00」「一次 · 11月23日 09:00」。
    pub fn label(&self) -> String {
        match self {
            Self::Every(every) => every_label(every.as_secs()),
            Self::Cron(expr) => describe_cron_zh(expr),
            Self::At(at) => format!("一次 · {}", local_label(*at, wall_ms())),
            Self::Watch(spec) => format!(
                "命令 · {}查一次，{}",
                every_label(spec.every.as_secs()),
                spec.when.label()
            ),
            Self::Files(spec) => format!("文件 · {} 有改动", clip(&spec.paths.join("、"), 48)),
            Self::Manual => "手动".into(),
        }
    }

    /// 任务面板那一行的短写：`5m` / `工作日 09:00` / `11月23日 09:00` / `5m $ gh pr checks`。
    pub fn short(&self) -> String {
        match self {
            Self::Every(every) => short_every(*every),
            Self::Cron(expr) => describe_cron_zh(expr),
            Self::At(at) => local_label(*at, wall_ms()),
            Self::Watch(spec) => {
                format!("{} $ {}", short_every(spec.every), clip(&spec.command, 32))
            }
            Self::Files(spec) => clip(&spec.paths.join(", "), 40),
            Self::Manual => "manual".into(),
        }
    }

    /// 任务面板那一行的标签：`Loop` / `Cron` / `Once` / `Watch` / `Files` / `Manual`。
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Every(_) => "Loop",
            Self::Cron(_) => "Cron",
            Self::At(_) => "Once",
            Self::Watch(_) => "Watch",
            Self::Files(_) => "Files",
            Self::Manual => "Manual",
        }
    }
}

fn short_every(every: Duration) -> String {
    let secs = every.as_secs().max(1);
    if secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// 按字符截断，超出的补省略号。
pub(super) fn clip(s: &str, max: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut out: String = one_line.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

fn wall_ms() -> u64 {
    super::now_ms()
}

/// 5 段 cron（分 时 日 月 周）。秒、年都不收：最细到分钟，所以两次触发至少隔 60 秒。
pub(super) fn parse_cron(expr: &str) -> Result<croner::Cron, CronError> {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(expr)
        .map_err(|e| {
            CronError::InvalidSchedule(format!(
                "cron 表达式 {expr:?} 不对：{e}（要 5 段：分 时 日 月 周，例如 \"0 9 * * 1-5\" 是工作日 9 点）"
            ))
        })
}

/// `after_ms` 之后（不含）第一次触发的墙钟毫秒；按本机时区算，夏令时由 croner 处理。
pub(super) fn cron_next_after(expr: &str, after_ms: u64) -> Option<u64> {
    let cron = parse_cron(expr).ok()?;
    let start = local_at(after_ms)?;
    let next = cron.find_next_occurrence(&start, false).ok()?;
    u64::try_from(next.timestamp_millis()).ok()
}

fn local_at(ms: u64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(i64::try_from(ms).ok()?).single()
}

/// 「只跑一次」的时刻：`30m` / `2h` 这种延迟，`2026-11-23 09:00`（本机时区）、
/// `2026-11-23T09:00:00+08:00`（RFC 3339），或者只给 `09:00`（今天，过了就明天）。
/// 必须在 `now_ms` 之后。
pub fn parse_at(raw: &str, now_ms: u64) -> Result<u64, CronError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(CronError::InvalidSchedule("at 不能为空".into()));
    }
    let bad = || {
        CronError::InvalidSchedule(format!(
            "看不懂时间 {s:?}：写延迟（30m、2h）或本机时间（2026-11-23 09:00、09:00）"
        ))
    };
    let at = if s.ends_with(['s', 'm', 'h', 'd'])
        && s[..s.len() - 1].bytes().all(|b| b.is_ascii_digit())
    {
        let secs = parse_interval(s).map_err(|e| CronError::InvalidSchedule(e.to_string()))?;
        now_ms.saturating_add(secs.saturating_mul(1000))
    } else if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        u64::try_from(dt.timestamp_millis()).map_err(|_| bad())?
    } else if let Some(naive) = parse_naive(s) {
        local_ms(naive, s)?
    } else if let Some(time) = parse_clock(s) {
        let today = local_at(now_ms).ok_or_else(bad)?.date_naive();
        let mut at = local_ms(today.and_time(time), s)?;
        if at <= now_ms {
            let tomorrow = today.succ_opt().ok_or_else(bad)?;
            at = local_ms(tomorrow.and_time(time), s)?;
        }
        at
    } else {
        return Err(bad());
    };
    if at <= now_ms {
        return Err(CronError::InvalidSchedule(format!("时间 {s} 已经过了")));
    }
    Ok(at)
}

fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    const FORMATS: [&str; 6] = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
    ];
    FORMATS
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
}

fn parse_clock(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
        .ok()
}

/// 本机时区的一个钟面时刻。回拨重复的那一小时取前一次；跳过的那一小时没有这个时刻，报错。
fn local_ms(naive: NaiveDateTime, raw: &str) -> Result<u64, CronError> {
    let dt = match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt,
        LocalResult::Ambiguous(first, _) => first,
        LocalResult::None => {
            return Err(CronError::InvalidSchedule(format!(
                "本机时区没有 {raw} 这个时刻（夏令时跳过了）"
            )))
        }
    };
    u64::try_from(dt.timestamp_millis())
        .map_err(|_| CronError::InvalidSchedule(format!("时间 {raw} 不对")))
}

fn local_stamp(ms: u64) -> String {
    local_at(ms)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// 「11月23日 09:00」；不是今年就带上年份。
fn local_label(ms: u64, now_ms: u64) -> String {
    let Some(dt) = local_at(ms) else {
        return ms.to_string();
    };
    let same_year = local_at(now_ms).is_some_and(|now| now.year() == dt.year());
    let day = format!("{}月{}日 {}", dt.month(), dt.day(), dt.format("%H:%M"));
    if same_year {
        day
    } else {
        format!("{}年{day}", dt.year())
    }
}

fn every_label(secs: u64) -> String {
    let unit = |n: u64, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            format!("每 {n} {many}")
        }
    };
    if secs > 0 && secs.is_multiple_of(86_400) {
        unit(secs / 86_400, "每天", "天")
    } else if secs > 0 && secs.is_multiple_of(3600) {
        unit(secs / 3600, "每小时", "小时")
    } else if secs > 0 && secs.is_multiple_of(60) {
        unit(secs / 60, "每分钟", "分钟")
    } else {
        format!("每 {secs} 秒")
    }
}

/// 常见写法翻成中文，认不出的照原样：`cron 0 9 1-7 * 1`。
pub(super) fn describe_cron_zh(expr: &str) -> String {
    describe_known(expr.trim()).unwrap_or_else(|| format!("cron {}", expr.trim()))
}

fn describe_known(expr: &str) -> Option<String> {
    let alias = match expr.to_ascii_lowercase().as_str() {
        "@hourly" => Some("每小时整点"),
        "@daily" | "@midnight" => Some("每天 00:00"),
        "@weekly" => Some("每周日 00:00"),
        "@monthly" => Some("每月 1 日 00:00"),
        "@yearly" | "@annually" => Some("每年 1 月 1 日 00:00"),
        _ => None,
    };
    if let Some(alias) = alias {
        return Some(alias.to_string());
    }
    let fields: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else {
        return None;
    };
    if *month != "*" {
        return None;
    }
    let num = |s: &str| s.parse::<u32>().ok();
    let step = |s: &str| s.strip_prefix("*/").and_then(num);

    // 整点之外的分钟级：每 N 分钟、每小时第 M 分、每 N 小时。
    if *dom == "*" && *dow == "*" {
        if *minute == "*" && *hour == "*" {
            return Some("每分钟".into());
        }
        if let (Some(n), "*") = (step(minute), *hour) {
            return Some(format!("每 {n} 分钟"));
        }
        if let (Some(m), "*") = (num(minute), *hour) {
            return Some(if m == 0 {
                "每小时整点".into()
            } else {
                format!("每小时第 {m} 分")
            });
        }
        if let (Some(m), Some(n)) = (num(minute), step(hour)) {
            return Some(if m == 0 {
                format!("每 {n} 小时")
            } else {
                format!("每 {n} 小时（第 {m} 分）")
            });
        }
    }

    // 固定钟点：分钟是一个数，小时是一个数或一串数。
    let m = num(minute)?;
    let hours: Vec<u32> = hour.split(',').map(num).collect::<Option<_>>()?;
    if m > 59 || hours.iter().any(|h| *h > 23) {
        return None;
    }
    let times = hours
        .iter()
        .map(|h| format!("{h:02}:{m:02}"))
        .collect::<Vec<_>>()
        .join("、");

    match (*dom, *dow) {
        ("*", "*") => Some(format!("每天 {times}")),
        ("*", dow) => Some(format!("{} {times}", weekdays_zh(dow)?)),
        ("L", "*") => Some(format!("每月最后一天 {times}")),
        (dom, "*") => {
            let days: Vec<u32> = dom.split(',').map(num).collect::<Option<_>>()?;
            if days.iter().any(|d| *d == 0 || *d > 31) {
                return None;
            }
            let days = days
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("、");
            Some(format!("每月 {days} 日 {times}"))
        }
        _ => None,
    }
}

/// `1-5` → 工作日，`0,6` → 周末，`1,3,5` → 每周一、三、五。
fn weekdays_zh(dow: &str) -> Option<String> {
    let lower = dow.to_ascii_lowercase();
    match lower.as_str() {
        "1-5" | "mon-fri" => return Some("工作日".into()),
        "0,6" | "6,0" | "6,7" | "6-7" | "sat,sun" | "sun,sat" => return Some("周末".into()),
        "*" => return Some("每天".into()),
        _ => {}
    }
    const NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
    const ZH: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
    let days: Vec<usize> = lower
        .split(',')
        .map(|d| {
            d.parse::<usize>()
                .ok()
                .map(|n| if n == 7 { 0 } else { n })
                .filter(|n| *n < 7)
                .or_else(|| NAMES.iter().position(|name| *name == d))
        })
        .collect::<Option<_>>()?;
    let days = days.iter().map(|d| ZH[*d]).collect::<Vec<_>>().join("、");
    Some(format!("每周{days}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn cron_needs_five_fields() {
        assert!(CronSchedule::cron("0 9 * * 1-5").is_ok());
        assert!(matches!(
            CronSchedule::cron("0 0 9 * * 1-5"),
            Err(CronError::InvalidSchedule(_))
        ));
        assert!(matches!(
            CronSchedule::cron("every day"),
            Err(CronError::InvalidSchedule(_))
        ));
    }

    #[test]
    fn cron_next_lands_on_the_local_clock() {
        let now = super::super::now_ms();
        let next = cron_next_after("30 9 * * *", now).unwrap();
        assert!(next > now && next <= now + DAY_MS, "{now} → {next}");
        let local = local_at(next).unwrap();
        assert_eq!((local.hour(), local.minute(), local.second()), (9, 30, 0));
        // 从刚算出的那一刻往后找，是第二天（不含当刻）。
        let after = cron_next_after("30 9 * * *", next).unwrap();
        assert!(after > next);
        assert_eq!(local_at(after).unwrap().hour(), 9);
    }

    #[test]
    fn period_hint_takes_the_shortest_gap() {
        let now = super::super::now_ms();
        let weekdays = CronSchedule::cron("0 9 * * 1-5").unwrap();
        // 周一到周五隔一天，周五到周一隔三天：取一天。夏令时那天差一小时也算「一天上下」。
        let gap = weekdays.period_hint(now).unwrap().as_secs();
        assert!((23 * 3600..=25 * 3600).contains(&gap), "{gap}");
        let often = CronSchedule::cron("*/15 * * * *").unwrap();
        assert_eq!(often.period_hint(now), Some(Duration::from_secs(900)));
        assert_eq!(CronSchedule::At(now + 1000).period_hint(now), None);
    }

    #[test]
    fn describes_common_cron_in_chinese() {
        assert_eq!(describe_cron_zh("0 9 * * *"), "每天 09:00");
        assert_eq!(describe_cron_zh("30 8 * * 1-5"), "工作日 08:30");
        assert_eq!(describe_cron_zh("0 10 * * 1"), "每周一 10:00");
        assert_eq!(
            describe_cron_zh("0 10 * * MON,WED,FRI"),
            "每周一、三、五 10:00"
        );
        assert_eq!(describe_cron_zh("0 9,18 * * *"), "每天 09:00、18:00");
        assert_eq!(describe_cron_zh("0 9 1 * *"), "每月 1 日 09:00");
        assert_eq!(describe_cron_zh("0 9 L * *"), "每月最后一天 09:00");
        assert_eq!(describe_cron_zh("*/15 * * * *"), "每 15 分钟");
        assert_eq!(describe_cron_zh("0 */2 * * *"), "每 2 小时");
        assert_eq!(describe_cron_zh("5 * * * *"), "每小时第 5 分");
        assert_eq!(describe_cron_zh("0 0 * * 0,6"), "周末 00:00");
        assert_eq!(describe_cron_zh("@daily"), "每天 00:00");
        assert_eq!(describe_cron_zh("0 9 1-7 * 1"), "cron 0 9 1-7 * 1");
        assert_eq!(describe_cron_zh("0 9 * 1 *"), "cron 0 9 * 1 *");
    }

    #[test]
    fn watch_and_files_validate_and_describe() {
        // 短于一分钟的报错，不悄悄提到一分钟（#310 审阅）。
        assert!(matches!(
            CronSchedule::watch("gh pr checks 12", Duration::from_secs(10), WatchWhen::Fails),
            Err(CronError::InvalidSchedule(m)) if m.contains("每分钟")
        ));
        let watch =
            CronSchedule::watch("gh pr checks 12", MIN_CHECK_EVERY, WatchWhen::Fails).unwrap();
        assert_eq!(watch.label(), "命令 · 每分钟查一次，失败时触发");
        assert!(
            watch.human().contains("`gh pr checks 12` fails"),
            "{}",
            watch.human()
        );
        assert_eq!(watch.short(), "1m $ gh pr checks 12");
        assert_eq!(watch.period_hint(0), Some(MIN_CHECK_EVERY));
        assert!(matches!(
            CronSchedule::watch("  ", DEFAULT_CHECK_EVERY, WatchWhen::Changed),
            Err(CronError::InvalidSchedule(_))
        ));
        assert!(matches!(
            CronSchedule::watch(
                "curl x",
                DEFAULT_CHECK_EVERY,
                WatchWhen::Matches { pattern: "(".into() }
            ),
            Err(CronError::InvalidSchedule(m)) if m.contains("正则")
        ));

        let files = CronSchedule::files(vec![" docs/ ".into(), "".into()], Duration::ZERO).unwrap();
        let CronSchedule::Files(spec) = &files else {
            panic!("{files:?}");
        };
        assert_eq!(spec.paths, vec!["docs/".to_string()]);
        assert_eq!(spec.debounce, Duration::from_secs(1));
        assert_eq!(files.label(), "文件 · docs/ 有改动");
        assert!(CronSchedule::files(vec![], DEFAULT_FILES_DEBOUNCE).is_err());
        assert_eq!(CronSchedule::Manual.label(), "手动");
        assert_eq!(CronSchedule::Manual.period_hint(0), None);
    }

    #[test]
    fn every_labels() {
        assert_eq!(every_label(300), "每 5 分钟");
        assert_eq!(every_label(60), "每分钟");
        assert_eq!(every_label(3600), "每小时");
        assert_eq!(every_label(7200), "每 2 小时");
        assert_eq!(every_label(86_400), "每天");
        assert_eq!(every_label(259_200), "每 3 天");
        assert_eq!(every_label(90), "每 90 秒");
    }

    #[test]
    fn parse_at_accepts_delays_and_local_times() {
        let now = super::super::now_ms();
        assert_eq!(parse_at("30m", now).unwrap(), now + 1_800_000);
        assert_eq!(parse_at(" 2h ", now).unwrap(), now + 7_200_000);

        let tomorrow = local_at(now + DAY_MS).unwrap().date_naive();
        let raw = format!("{} 09:15", tomorrow.format("%Y-%m-%d"));
        let at = parse_at(&raw, now).unwrap();
        let local = local_at(at).unwrap();
        assert_eq!(local.date_naive(), tomorrow);
        assert_eq!((local.hour(), local.minute()), (9, 15));
        assert_eq!(parse_at(&raw.replace(' ', "T"), now).unwrap(), at);

        let rfc = local.to_rfc3339();
        assert_eq!(parse_at(&rfc, now).unwrap(), at);

        // 只给钟点：今天还没到就是今天，过了就是明天；总之在未来一天内。
        let clock = parse_at("09:15", now).unwrap();
        assert!(clock > now && clock <= now + DAY_MS);
    }

    #[test]
    fn parse_at_rejects_the_past_and_nonsense() {
        let now = super::super::now_ms();
        assert!(matches!(
            parse_at("2020-01-01 09:00", now),
            Err(CronError::InvalidSchedule(m)) if m.contains("已经过了")
        ));
        assert!(matches!(
            parse_at("明天早上", now),
            Err(CronError::InvalidSchedule(_))
        ));
        assert!(parse_at("", now).is_err());
    }
}

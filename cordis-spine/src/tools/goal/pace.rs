//! 目标模式怎么跑长：不设总上限，只在三种情况下停。
//!
//! - **空转**：连着 [`STALL_LIMIT`] 次催它继续，中间都没动手（没调任何干活的工具），
//!   自动暂停。真在干活的长任务不受影响。
//! - **自己收敛**：每 [`CHECKIN_EVERY`] 次请求插一条自查提醒，让模型对照完成标准，
//!   自己报完成 / 受阻，不用人来喊停。
//! - **预算**：用户按需给某个目标设上限（`/goal budget 2h` / `/goal budget 300`），
//!   默认不限；到了就暂停。

use std::time::Duration;

/// 连着几次续跑之前都没动手，就当空转、停下。
pub const STALL_LIMIT: u32 = 3;

/// 每用掉这么多次请求，提醒模型自查一次。
pub const CHECKIN_EVERY: u64 = 50;

/// 不算「动手」的工具：只是报进度、记清单，本身不推进目标。
pub fn counts_as_work(tool: &str) -> bool {
    !matches!(tool, "update_goal" | "todo_write")
}

/// 用户给目标设的上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalBudget {
    /// 最多这么多次请求（本页的采样次数）。
    Requests(u64),
    /// 最多跑这么久（暂停的时段不算）。
    Time(Duration),
}

impl GoalBudget {
    /// `300` / `300次` → 请求数；`90m` / `2h` / `1.5h` / `30min` → 时长。
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().to_ascii_lowercase();
        let raw = raw.trim_end_matches('次').trim();
        if let Ok(n) = raw.parse::<u64>() {
            return (n > 0).then_some(Self::Requests(n));
        }
        let (num, unit_secs) = [
            ("hours", 3600.0),
            ("hour", 3600.0),
            ("小时", 3600.0),
            ("min", 60.0),
            ("分钟", 60.0),
            ("h", 3600.0),
            ("m", 60.0),
        ]
        .iter()
        .find_map(|(suffix, secs)| raw.strip_suffix(suffix).map(|n| (n.trim(), *secs)))?;
        let n: f64 = num.parse().ok()?;
        let secs = n * unit_secs;
        (secs.is_finite() && secs >= 60.0).then(|| Self::Time(Duration::from_secs(secs as u64)))
    }

    pub fn reached(&self, requests: u64, elapsed: Duration) -> bool {
        match *self {
            Self::Requests(max) => requests >= max,
            Self::Time(max) => elapsed >= max,
        }
    }

    pub fn label(&self) -> String {
        match *self {
            Self::Requests(n) => format!("{n} 次请求"),
            Self::Time(d) => duration_label(d),
        }
    }
}

/// `1 小时 30 分钟` / `45 分钟` / `20 秒`。
pub fn duration_label(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    match (h, m) {
        (0, 0) => format!("{secs} 秒"),
        (0, m) => format!("{m} 分钟"),
        (h, 0) => format!("{h} 小时"),
        (h, m) => format!("{h} 小时 {m} 分钟"),
    }
}

/// 自查提醒：让模型把目标落成能检查的完成标准，对照着自己收尾。
pub fn checkin_reminder(objective: &str, requests: u64, elapsed: Duration) -> String {
    format!(
        "<system-reminder>\n\
         <goal-checkin>\n\
         Objective: {objective}\n\
         这个目标已经用了 {requests} 次请求、跑了 {}。\n\
         </goal-checkin>\n\
         停一下，对照目标自查（不用问用户）：\n\
         1. 写出这个目标的完成标准：几条能检查的事实，不是感觉。\n\
         2. 逐条看现在满足没有，有证据就引用（文件、命令输出、页面）。\n\
         3. 都满足了：调用 update_goal(completed: true, message: 总结)。\n\
         卡住了、同一个问题试了 3 次以上：调用 update_goal(blocked_reason: …)。\n\
         还差：用 update_goal(message: …) 写一句还差什么，然后直接接着做最要紧的那条。\n\
         别为了更完美无限打磨：标准满足就收尾。\n\
         </system-reminder>",
        duration_label(elapsed)
    )
}

/// 续跑之前查一下该不该停：返回停下的原因（写进目标进展，给人看）。
pub fn stop_reason(
    idle_nudges: u32,
    worked_since_nudge: bool,
    budget: Option<GoalBudget>,
    requests: u64,
    elapsed: Duration,
) -> Option<String> {
    if let Some(budget) = budget.filter(|b| b.reached(requests, elapsed)) {
        return Some(format!(
            "已到预算（{}）：用了 {requests} 次请求、{}。目标已暂停，/goal resume 继续（不再限）。",
            budget.label(),
            duration_label(elapsed)
        ));
    }
    // 这次动过手，前面的空转就不算数了（和 `GoalState::nudged` 的结算一致）。
    let idle = if worked_since_nudge {
        0
    } else {
        idle_nudges + 1
    };
    (idle >= STALL_LIMIT).then(|| {
        format!(
            "连续 {idle} 次催它继续都没有动手（没调用干活的工具），像是在空转，目标已暂停。/goal resume 继续，或者发消息说下一步。"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_parse_as_requests_or_time() {
        assert_eq!(GoalBudget::parse("300"), Some(GoalBudget::Requests(300)));
        assert_eq!(GoalBudget::parse("300次"), Some(GoalBudget::Requests(300)));
        let hours = |h: u64| Some(GoalBudget::Time(Duration::from_secs(h * 3600)));
        assert_eq!(GoalBudget::parse("2h"), hours(2));
        assert_eq!(GoalBudget::parse("2 小时"), hours(2));
        assert_eq!(
            GoalBudget::parse("1.5h"),
            Some(GoalBudget::Time(Duration::from_secs(5400)))
        );
        assert_eq!(
            GoalBudget::parse("90m"),
            Some(GoalBudget::Time(Duration::from_secs(5400)))
        );
        assert_eq!(
            GoalBudget::parse("30min"),
            Some(GoalBudget::Time(Duration::from_secs(1800)))
        );
        for bad in ["", "0", "abc", "10s", "0.5m", "-1h"] {
            assert_eq!(GoalBudget::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn work_keeps_it_going_and_idle_stops_it() {
        let t = Duration::from_secs(60);
        assert_eq!(
            stop_reason(0, true, None, 5000, t),
            None,
            "在干活就不停，不管多久"
        );
        assert_eq!(stop_reason(STALL_LIMIT, true, None, 5, t), None);
        assert_eq!(stop_reason(STALL_LIMIT - 2, false, None, 5, t), None);
        let stall = stop_reason(STALL_LIMIT - 1, false, None, 5, t).expect("空转该停");
        assert!(stall.contains("空转"), "{stall}");
    }

    #[test]
    fn a_budget_stops_even_a_busy_goal() {
        let t = Duration::from_secs(60);
        let budget = Some(GoalBudget::Requests(100));
        assert_eq!(stop_reason(0, true, budget, 99, t), None);
        let hit = stop_reason(0, true, budget, 100, t).expect("到预算该停");
        assert!(hit.contains("100 次请求"), "{hit}");
        let time = Some(GoalBudget::Time(Duration::from_secs(3600)));
        assert_eq!(
            stop_reason(0, true, time, 1, Duration::from_secs(3599)),
            None
        );
        assert!(stop_reason(0, true, time, 1, Duration::from_secs(3600)).is_some());
    }

    #[test]
    fn progress_tools_alone_are_not_work() {
        assert!(!counts_as_work("update_goal"));
        assert!(!counts_as_work("todo_write"));
        assert!(counts_as_work("bash"));
        assert!(counts_as_work("mcp_browser__browser_js"));
    }
}

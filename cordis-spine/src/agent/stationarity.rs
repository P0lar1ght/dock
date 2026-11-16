//! 连续相同步检测：模型原地打转时先提醒，还不改就停下这一轮。
//!
//! 循环本身没有步数上限：grok-build 的 `max_turns` 默认不限，Codex 也不设。兜底的
//! 是这里，照 grok-build 的 `IdenticalToolCallRun`
//! （`xai-grok-shell/src/session/acp_session_impl/turn.rs`）：
//!
//! - 一步的签名 = 每颗调用「名字 + 键排序后的参数 JSON」，再排序拼起来。参数里键的
//!   顺序、同一步里几颗调用的顺序都不算变化。
//! - 两档阈值：一步里全是 [`TIGHT_TOOLS`]，第 4 次提醒、第 8 次停；别的第 8 次提醒、
//!   第 12 次停。只有一颗 `bash true` 的空转第 4 次直接停，不提醒。
//! - 第 N 次相同的那一步，工具结果全部落完之后才判断：提醒或停下都不会夹在
//!   `tool_calls` 和它的结果中间。
//!
//! 抓不到 A、B 两步来回切换的空转，两家也都不管：主会话可以 Esc，子代理可以在任务
//! 面板里停。

use std::hash::{Hash, Hasher};

use cordis_base::types::ToolCall;

/// 原样重复几乎不会有进展的工具：读同一个文件、写同一份清单。
///
/// grok-build 按工具种类 `Read | Plan` 判断，注释里记着线上一轮把同一个
/// `todo_write` 原样重复了 12 次（那一轮一共调了 224 次）。搜索、列目录、查后台
/// 任务都在宽档。
const TIGHT_TOOLS: &[&str] = &["read_file", "todo_write"];
const NUDGE_TIGHT: u32 = 4;
const STOP_TIGHT: u32 = 8;
const NUDGE: u32 = 8;
const STOP: u32 = 12;
const STOP_TRUE_NOOP: u32 = 4;
const _: () = assert!(NUDGE_TIGHT < STOP_TIGHT);
const _: () = assert!(NUDGE < STOP);

/// 一步工具调用的身份。要在调用交给执行之前算：执行会把它们拿走。
pub(crate) struct Step {
    signature: u64,
    /// 提醒和停下的说明里报的名字：这一步里字典序最小的工具名，换序不变。
    tool: String,
    tight: bool,
    true_noop: bool,
}

impl Step {
    pub(crate) fn of(calls: &[ToolCall]) -> Self {
        let mut parts: Vec<String> = calls
            .iter()
            .map(|c| {
                let args = serde_json::from_str::<serde_json::Value>(&c.arguments)
                    .map(|v| canonical(v).to_string())
                    .unwrap_or_else(|_| c.arguments.trim().to_string());
                format!("{}\u{1f}{args}", c.name)
            })
            .collect();
        parts.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        parts.join("\u{1e}").hash(&mut hasher);
        Self {
            signature: hasher.finish(),
            tool: calls
                .iter()
                .map(|c| c.name.as_str())
                .min()
                .unwrap_or_default()
                .to_string(),
            tight: !calls.is_empty()
                && calls.iter().all(|c| TIGHT_TOOLS.contains(&c.name.as_str())),
            true_noop: matches!(calls, [c] if c.name == "bash"
                && crate::tools::read_only::bash_command(&c.arguments).trim().eq_ignore_ascii_case("true")),
        }
    }
}

/// 键排好序的 JSON：同一份参数不管模型按什么顺序写键，序列化出来都一样。
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Go,
    /// 落这条 `<system-reminder>` 再接着跑。同一串重复只提醒一次。
    Nudge(String),
    /// 停下这一轮。
    Stop {
        tool: String,
        run: u32,
    },
}

/// 连着几步一模一样。一轮一份，放在循环的局部变量里。
#[derive(Default)]
pub(crate) struct IdenticalSteps {
    last: Option<u64>,
    run: u32,
    nudged: bool,
}

/// 只有一颗 `bash true` 的步不管参数里别的字段写了什么，都算同一种空转。
const TRUE_NOOP_KEY: u64 = 0;

impl IdenticalSteps {
    /// 这一步的工具结果都落完之后调用。
    pub(crate) fn observe(&mut self, step: Step) -> Verdict {
        let key = if step.true_noop {
            TRUE_NOOP_KEY
        } else {
            step.signature
        };
        if self.last == Some(key) {
            self.run += 1;
        } else {
            self.last = Some(key);
            self.run = 1;
            self.nudged = false;
        }
        let (nudge, stop) = if step.true_noop {
            (None, STOP_TRUE_NOOP)
        } else if step.tight {
            (Some(NUDGE_TIGHT), STOP_TIGHT)
        } else {
            (Some(NUDGE), STOP)
        };
        if self.run >= stop {
            return Verdict::Stop {
                tool: step.tool,
                run: self.run,
            };
        }
        if !self.nudged && nudge.is_some_and(|n| self.run >= n) {
            self.nudged = true;
            return Verdict::Nudge(nudge_reminder(&step.tool, self.run));
        }
        Verdict::Go
    }

    /// 模型回了正文：这一段到头了，turn-end 续跑之后重新数。
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

fn nudge_reminder(tool: &str, run: u32) -> String {
    format!(
        "<system-reminder>\n你已经连着 {run} 步用完全相同的参数调用 `{tool}`，看起来卡在了循环里。\
         别再重复这一步。在等后台命令或任务的话，它跑完会自动通知你，不用反复查。没法推进就停下，\
         告诉用户你在等什么。同样的调用再重复下去，这一轮会被自动停下。\n</system-reminder>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: format!("c-{name}"),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    fn run(steps: impl IntoIterator<Item = Vec<ToolCall>>) -> Vec<Verdict> {
        let mut identical = IdenticalSteps::default();
        steps
            .into_iter()
            .map(|calls| identical.observe(Step::of(&calls)))
            .collect()
    }

    fn first_nudge_and_stop(verdicts: &[Verdict]) -> (Option<usize>, Option<usize>) {
        (
            verdicts
                .iter()
                .position(|v| matches!(v, Verdict::Nudge(_)))
                .map(|i| i + 1),
            verdicts
                .iter()
                .position(|v| matches!(v, Verdict::Stop { .. }))
                .map(|i| i + 1),
        )
    }

    #[test]
    fn reading_the_same_file_nudges_at_4_and_stops_at_8() {
        let verdicts = run((0..8).map(|_| vec![call("read_file", r#"{"path":"a.rs"}"#)]));
        assert_eq!(first_nudge_and_stop(&verdicts), (Some(4), Some(8)));
        assert_eq!(
            verdicts
                .iter()
                .filter(|v| matches!(v, Verdict::Nudge(_)))
                .count(),
            1,
            "同一串重复只提醒一次"
        );
        assert_eq!(
            verdicts[7],
            Verdict::Stop {
                tool: "read_file".into(),
                run: 8
            }
        );
    }

    #[test]
    fn other_tools_nudge_at_8_and_stop_at_12() {
        let verdicts = run((0..12).map(|_| vec![call("grep", r#"{"pattern":"x"}"#)]));
        assert_eq!(first_nudge_and_stop(&verdicts), (Some(8), Some(12)));
    }

    #[test]
    fn a_mixed_step_is_in_the_loose_tier() {
        let step = || {
            vec![
                call("read_file", r#"{"path":"a"}"#),
                call("bash", r#"{"command":"ls"}"#),
            ]
        };
        let verdicts = run((0..12).map(|_| step()));
        assert_eq!(first_nudge_and_stop(&verdicts), (Some(8), Some(12)));
    }

    #[test]
    fn bash_true_stops_at_4_without_a_nudge() {
        let verdicts = run((0..4).map(|i| {
            // 描述不一样也算同一种空转。
            vec![call(
                "bash",
                &format!(r#"{{"command":" true ","description":"keepalive {i}"}}"#),
            )]
        }));
        assert_eq!(first_nudge_and_stop(&verdicts), (None, Some(4)));
    }

    #[test]
    fn key_order_and_call_order_do_not_count_as_progress() {
        let a = call("read_file", r#"{"path":"a","limit":10}"#);
        let a_reordered = call("read_file", r#"{"limit":10,"path":"a"}"#);
        let b = call("grep", r#"{"pattern":"x"}"#);
        let mut identical = IdenticalSteps::default();
        identical.observe(Step::of(&[a.clone(), b.clone()]));
        identical.observe(Step::of(&[b, a_reordered]));
        assert_eq!(identical.run, 2);
    }

    #[test]
    fn a_different_step_starts_the_count_over() {
        let same = || vec![call("read_file", r#"{"path":"a"}"#)];
        let mut steps: Vec<Vec<ToolCall>> = (0..7).map(|_| same()).collect();
        steps.push(vec![call("read_file", r#"{"path":"b"}"#)]);
        steps.extend((0..7).map(|_| same()));
        let verdicts = run(steps);
        assert!(
            !verdicts.iter().any(|v| matches!(v, Verdict::Stop { .. })),
            "中间换过一步就重新数：{verdicts:?}"
        );
        assert_eq!(
            verdicts
                .iter()
                .filter(|v| matches!(v, Verdict::Nudge(_)))
                .count(),
            2,
            "新的一串重复到门槛再提醒一次"
        );
    }

    #[test]
    fn reset_starts_the_count_over() {
        let mut identical = IdenticalSteps::default();
        for _ in 0..7 {
            identical.observe(Step::of(&[call("read_file", r#"{"path":"a"}"#)]));
        }
        identical.reset();
        assert_eq!(
            identical.observe(Step::of(&[call("read_file", r#"{"path":"a"}"#)])),
            Verdict::Go
        );
        assert_eq!(identical.run, 1);
    }
}

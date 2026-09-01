//! Per-call and per-session token ledger (not serialized).
//!
//! Copied from grok-build `xai-chat-state::usage` + pager
//! `session_usage_block_text`. Account quota / grok.com billing is not here.
//!
//! `total_tokens()` is input + output. Compaction and other side calls must
//! not call [`UsageLedger::record_main_loop_call`].
//!
//! Partial costs are scrubbed (absence ≠ free). Totals reset when the live
//! session is cleared or restored.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// One model call's tokens. Copied from grok-build `xai-grok-sampling-types::TokenUsage`.
///
/// `prompt_tokens` is the FULL prompt (uncached + cache reads + cache writes).
/// `cached_prompt_tokens` is only the cache-hit subset; do not subtract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub reasoning_tokens: u64,
    pub cached_prompt_tokens: u64,
    pub cache_creation_prompt_tokens: u64,
}

/// Normalize a wire cost-ticks value at capture.
///
/// The REST layer backfills `0` for unreported cost, and negative ticks are
/// never valid, so both become `None` ("unreported", never "free").
pub fn reported_cost_ticks(raw: Option<i64>) -> Option<i64> {
    raw.filter(|&t| t > 0)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
    pub model_calls: u64,
    pub api_duration_ms: u64,
    /// USD ticks (1e10 per USD). Absent when no call reported cost.
    pub cost_usd_ticks: Option<i64>,
    pub cost_missing_calls: u64,
}

impl UsageTotals {
    fn from_call(
        usage: &TokenUsage,
        api_duration_ms: Option<u64>,
        cost_usd_ticks: Option<i64>,
    ) -> Self {
        let cost_usd_ticks = reported_cost_ticks(cost_usd_ticks);
        Self {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cached_read_tokens: usage.cached_prompt_tokens,
            cache_creation_tokens: usage.cache_creation_prompt_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            model_calls: 1,
            api_duration_ms: api_duration_ms.unwrap_or(0),
            cost_usd_ticks,
            cost_missing_calls: u64::from(cost_usd_ticks.is_none()),
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn cost_is_partial(&self) -> bool {
        self.cost_usd_ticks.is_some() && self.cost_missing_calls > 0
    }

    fn fold_totals(&mut self, other: &UsageTotals) {
        let Self {
            input_tokens,
            output_tokens,
            cached_read_tokens,
            cache_creation_tokens,
            reasoning_tokens,
            model_calls,
            api_duration_ms,
            cost_usd_ticks,
            cost_missing_calls,
        } = other;
        self.input_tokens = self.input_tokens.saturating_add(*input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(*output_tokens);
        self.cached_read_tokens = self.cached_read_tokens.saturating_add(*cached_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(*cache_creation_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(*reasoning_tokens);
        self.model_calls = self.model_calls.saturating_add(*model_calls);
        self.api_duration_ms = self.api_duration_ms.saturating_add(*api_duration_ms);
        self.cost_missing_calls = self.cost_missing_calls.saturating_add(*cost_missing_calls);
        self.cost_usd_ticks = merge_cost_ticks(self.cost_usd_ticks, *cost_usd_ticks);
    }
}

fn merge_cost_ticks(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageLedger {
    pub totals: UsageTotals,
    pub by_model: IndexMap<String, UsageTotals>,
    /// Main-agent loop rounds for `num_turns` (subagents excluded).
    pub main_loop_model_calls: u64,
    /// Last main-loop call (for `/usage` 上一轮命中). Subagents do not overwrite.
    pub last_call: Option<UsageTotals>,
    /// Bill may under-count (drain timeout, nested subagent incomplete, apply failure).
    pub incomplete: bool,
}

impl UsageLedger {
    /// Fold one main-agent-loop model call. This is the only writer of
    /// `main_loop_model_calls` (the wire `numTurns`); side calls such as
    /// compaction must not use it.
    pub fn record_main_loop_call(
        &mut self,
        model_id: &str,
        usage: &TokenUsage,
        api_duration_ms: Option<u64>,
        cost_usd_ticks: Option<i64>,
    ) {
        let call = UsageTotals::from_call(usage, api_duration_ms, cost_usd_ticks);
        self.main_loop_model_calls = self.main_loop_model_calls.saturating_add(1);
        self.last_call = Some(call.clone());
        self.fold_entry(model_id, &call);
    }

    /// Fold subagent usage without incrementing `main_loop_model_calls`.
    pub fn record_subagent(&mut self, by_model: &[(String, UsageTotals)], incomplete: bool) {
        for (model_id, totals) in by_model {
            self.fold_entry(model_id, totals);
        }
        if incomplete {
            self.incomplete = true;
        }
    }

    pub fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    fn fold_entry(&mut self, model_id: &str, totals: &UsageTotals) {
        self.totals.fold_totals(totals);
        self.by_model
            .entry(model_id.to_owned())
            .or_default()
            .fold_totals(totals);
    }
}

/// Wire usage for `/usage`. Copied from grok-build `PromptUsage` (ACP shape,
/// without the ACP client). Cost is only shown when complete and not partial.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptUsage {
    #[serde(flatten)]
    pub totals: PromptUsageModel,
    #[serde(
        default,
        rename = "modelUsage",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub model_usage: IndexMap<String, PromptUsageModel>,
    /// Main-agent loop rounds (same unit as `--max-turns`).
    #[serde(default, rename = "numTurns")]
    pub num_turns: u64,
    /// Bill may under-count (open subagents, usage not applied, or drain timeout).
    #[serde(
        default,
        rename = "usageIsIncomplete",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub usage_is_incomplete: bool,
    /// Last main-agent-loop call. Session totals stay cumulative.
    #[serde(default, rename = "lastCall", skip_serializing_if = "Option::is_none")]
    pub last_call: Option<PromptUsageModel>,
}

impl PromptUsage {
    /// Drop cost ticks when partial or incomplete so all surfaces fail closed.
    pub fn scrub_untrustworthy_costs(&mut self) {
        if !(self.usage_is_incomplete || self.totals.cost_is_partial) {
            return;
        }
        self.totals.cost_usd_ticks = None;
        for m in self.model_usage.values_mut() {
            m.cost_usd_ticks = None;
            if self.totals.cost_is_partial {
                m.cost_is_partial = true;
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptUsageModel {
    /// Full prompt input tokens including cache reads.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cached_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub model_calls: u64,
    #[serde(default)]
    pub api_duration_ms: u64,
    /// Server cost in USD ticks (`USD_TICKS_PER_USD` = 1e10 ticks per $1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd_ticks: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cost_is_partial: bool,
    #[serde(default, skip_serializing)]
    pub cost_missing_calls: u64,
}

/// Server cost scale: 1 USD = 10^10 ticks.
pub const USD_TICKS_PER_USD: f64 = 1e10;

pub fn ticks_to_usd(ticks: i64) -> f64 {
    ticks as f64 / USD_TICKS_PER_USD
}

impl From<&UsageTotals> for PromptUsageModel {
    fn from(t: &UsageTotals) -> Self {
        let UsageTotals {
            input_tokens,
            output_tokens,
            cached_read_tokens,
            cache_creation_tokens,
            reasoning_tokens,
            model_calls,
            api_duration_ms,
            cost_usd_ticks,
            cost_missing_calls,
        } = *t;
        Self {
            input_tokens,
            output_tokens,
            total_tokens: t.total_tokens(),
            cached_read_tokens,
            cache_creation_tokens,
            reasoning_tokens,
            model_calls,
            api_duration_ms,
            cost_usd_ticks,
            cost_is_partial: t.cost_is_partial(),
            cost_missing_calls,
        }
    }
}

impl From<&UsageLedger> for PromptUsage {
    fn from(ledger: &UsageLedger) -> Self {
        let mut usage = Self {
            totals: PromptUsageModel::from(&ledger.totals),
            model_usage: ledger
                .by_model
                .iter()
                .map(|(k, v)| (k.clone(), PromptUsageModel::from(v)))
                .collect(),
            num_turns: ledger.main_loop_model_calls,
            usage_is_incomplete: ledger.incomplete,
            last_call: ledger.last_call.as_ref().map(PromptUsageModel::from),
        };
        usage.scrub_untrustworthy_costs();
        usage
    }
}

/// `/usage` body — per-session token and cost totals, scoped to the ledger's
/// lifetime: since session start, or since the last `/resume`.
///
/// Layout copied from grok pager `session_usage_block_text`; labels are Chinese.
pub fn session_usage_block_text(usage: &PromptUsage) -> String {
    let t = &usage.totals;
    if t.model_calls == 0 && usage.model_usage.is_empty() {
        return if usage.usage_is_incomplete {
            "本会话用量：尚未记录，但统计不完整，可能少计。".to_string()
        } else {
            "本会话用量：尚未有模型调用。".to_string()
        };
    }

    let mut rows = Vec::new();
    rows.push(format!(
        "  输入 token:    {}（缓存 {} · {}）",
        group_thousands(t.input_tokens),
        group_thousands(t.cached_read_tokens),
        share_percent(t.cached_read_tokens, t.input_tokens),
    ));
    if let Some(last) = &usage.last_call {
        rows.push(format!(
            "  上一轮命中:    {} / {} · {}",
            group_thousands(last.cached_read_tokens),
            group_thousands(last.input_tokens),
            share_percent(last.cached_read_tokens, last.input_tokens),
        ));
    }
    rows.push(format!(
        "  输出 token:    {}（思考 {}）",
        group_thousands(t.output_tokens),
        group_thousands(t.reasoning_tokens),
    ));
    rows.push(format!(
        "  合计 token:    {}",
        group_thousands(t.total_tokens)
    ));
    rows.push(format!(
        "  模型调用:      {} · API 耗时: {}",
        group_thousands(t.model_calls),
        format_duration(Duration::from_millis(t.api_duration_ms)),
    ));
    rows.push(format!("  费用:          {}", format_cost(t)));

    if usage.model_usage.len() > 1 {
        rows.push("  按模型:".to_string());
        for (model, m) in &usage.model_usage {
            rows.push(format!(
                "    {model}: {} 入 / {} 出 · 缓存 {} · {}",
                group_thousands(m.input_tokens),
                group_thousands(m.output_tokens),
                share_percent(m.cached_read_tokens, m.input_tokens),
                format_cost(m),
            ));
        }
    }

    if usage.usage_is_incomplete {
        rows.push("  说明：用量统计不完整，可能少计。".to_string());
    }

    join_header_rows("本会话用量（自开始或上次恢复）：".to_string(), rows)
}

/// Copied from grok `xai-token-estimation::usage_percentage`.
/// `cached_prompt_tokens` is a subset of full `prompt_tokens` — do not subtract.
pub fn usage_percentage(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        ((used as f64) / (total as f64) * 100.0).min(100.0)
    }
}

/// Copied from grok pager `percent_of_window`. Tiny nonzero shares floor at
/// `0.1%`; `part` is clamped to `total` so a cache subset cannot read over 100%.
fn share_percent(part: u64, total: u64) -> String {
    if total == 0 {
        return "-".to_string();
    }
    let part = part.min(total);
    let p = ((part as f64 / total as f64) * 100.0).max(if part > 0 { 0.1 } else { 0.0 });
    if p < 10.0 {
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

fn format_cost(m: &PromptUsageModel) -> String {
    match m.cost_usd_ticks {
        Some(ticks) => format!("${:.4}", ticks_to_usd(ticks)),
        None if m.cost_is_partial => "未上报（部分调用无费用）".to_string(),
        None => "未上报".to_string(),
    }
}

fn join_header_rows(header: String, rows: Vec<String>) -> String {
    std::iter::once(header)
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compact duration: `5.2s`, `32s`, `2m5s`, `1h2m`. Copied from grok pager-render.
pub fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    if total_secs < 10 {
        return format!("{:.1}s", d.as_secs_f64());
    }
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins < 60 {
        return format!("{mins}m{secs}s");
    }
    let hours = mins / 60;
    let remaining_mins = mins % 60;
    format!("{hours}h{remaining_mins}m")
}

/// Group a count's digits with commas: `1234567` → `"1,234,567"`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tu(prompt: u64, completion: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: 999_999,
            reasoning_tokens: 0,
            cached_prompt_tokens: 0,
            cache_creation_prompt_tokens: 0,
        }
    }

    fn model_row(input: u64, output: u64, ticks: Option<i64>) -> PromptUsageModel {
        PromptUsageModel {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            cached_read_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
            model_calls: 1,
            api_duration_ms: 1_000,
            cost_usd_ticks: ticks,
            cost_is_partial: false,
            cost_missing_calls: 0,
        }
    }

    #[test]
    fn ledger_sums_partial_subagent_and_zero_cost() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("m", &tu(1, 1), None, Some(0));
        assert_eq!(ledger.totals.cost_usd_ticks, None);
        assert_eq!(ledger.totals.cost_missing_calls, 1);

        ledger.record_main_loop_call("a", &tu(100, 10), Some(100), None);
        ledger.record_main_loop_call("a", &tu(50, 5), Some(50), Some(70));
        assert_eq!(ledger.totals.cost_usd_ticks, Some(70));
        assert!(ledger.totals.cost_is_partial());
        assert_eq!(ledger.main_loop_model_calls, 3);

        ledger.record_subagent(
            &[(
                "b".into(),
                UsageTotals {
                    input_tokens: 5,
                    model_calls: 1,
                    ..Default::default()
                },
            )],
            false,
        );
        assert_eq!(ledger.by_model["b"].input_tokens, 5);
        assert_eq!(ledger.main_loop_model_calls, 3);
        assert_eq!(ledger.totals.model_calls, 4);
        assert!(!ledger.incomplete);

        ledger.record_subagent(&[], true);
        assert!(ledger.incomplete);
    }

    #[test]
    fn response_serializes_ledger_as_prompt_usage_wire_shape() {
        let mut ledger = UsageLedger::default();
        let mut call = tu(100, 10);
        call.cached_prompt_tokens = 40;
        call.reasoning_tokens = 3;
        ledger.record_main_loop_call("grok-build", &call, Some(50), Some(20_000_000));
        let v = serde_json::to_value(&PromptUsage::from(&ledger)).unwrap();
        assert_eq!(v["inputTokens"], 100);
        assert_eq!(v["outputTokens"], 10);
        assert_eq!(v["cachedReadTokens"], 40);
        assert_eq!(v["reasoningTokens"], 3);
        assert_eq!(v["numTurns"], 1);
        assert_eq!(v["costUsdTicks"], 20_000_000);
        assert_eq!(v["modelUsage"]["grok-build"]["inputTokens"], 100);
    }

    #[test]
    fn response_scrubs_partial_costs() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("a", &tu(100, 10), None, Some(70));
        ledger.record_main_loop_call("a", &tu(50, 5), None, None);
        let v = serde_json::to_value(&PromptUsage::from(&ledger)).unwrap();
        assert_eq!(v["costUsdTicks"], serde_json::Value::Null);
        assert_eq!(v["costIsPartial"], true);
    }

    #[test]
    fn session_usage_block_empty_ledger() {
        let usage = PromptUsage::default();
        assert_eq!(
            session_usage_block_text(&usage),
            "本会话用量：尚未有模型调用。"
        );

        let incomplete = PromptUsage {
            usage_is_incomplete: true,
            ..Default::default()
        };
        assert!(session_usage_block_text(&incomplete).contains("不完整"));
    }

    #[test]
    fn session_usage_block_shows_last_call_cache() {
        let mut last = model_row(13_000, 200, None);
        last.cached_read_tokens = 12_000;
        let mut totals = model_row(26_000, 400, None);
        totals.cached_read_tokens = 12_000;
        let usage = PromptUsage {
            totals,
            last_call: Some(last),
            ..Default::default()
        };
        let text = session_usage_block_text(&usage);
        assert!(
            text.contains("上一轮命中:    12,000 / 13,000 · 92%"),
            "{text}"
        );
    }

    #[test]
    fn session_usage_block_formats_tokens_and_cost() {
        let mut totals = model_row(1_234_567, 45_678, Some(12_345_000_000));
        totals.cached_read_tokens = 1_000_000;
        totals.reasoning_tokens = 12_000;
        totals.model_calls = 42;
        totals.api_duration_ms = 192_000;
        let usage = PromptUsage {
            totals,
            ..Default::default()
        };
        let text = session_usage_block_text(&usage);
        assert!(text.contains("1,234,567"), "{text}");
        assert!(text.contains("缓存 1,000,000 · 81%"), "{text}");
        assert!(!text.contains("上一轮命中"), "{text}");
        assert!(text.contains("思考 12,000"), "{text}");
        assert!(text.contains("$1.2345"), "{text}");
        assert!(text.contains("3m12s"), "{text}");
        assert!(!text.contains("按模型"), "{text}");
    }

    #[test]
    fn session_usage_block_lists_models_when_multiple() {
        let mut usage = PromptUsage {
            totals: model_row(150, 15, None),
            ..Default::default()
        };
        usage
            .model_usage
            .insert("grok-build".into(), model_row(100, 10, None));
        usage
            .model_usage
            .insert("grok-4".into(), model_row(50, 5, None));
        let text = session_usage_block_text(&usage);
        assert!(text.contains("按模型:"), "{text}");
        assert!(text.contains("grok-build: 100 入 / 10 出 · 缓存"), "{text}");
        assert!(text.contains("grok-4: 50 入 / 5 出 · 缓存"), "{text}");
    }

    #[test]
    fn session_usage_block_absent_cost_is_unknown_not_free() {
        let usage = PromptUsage {
            totals: model_row(100, 10, None),
            ..Default::default()
        };
        let text = session_usage_block_text(&usage);
        assert!(text.contains("未上报"), "{text}");
        assert!(!text.contains("$0"), "{text}");
    }

    #[test]
    fn usage_percentage_clamps_and_handles_zero_total() {
        assert_eq!(usage_percentage(0, 0), 0.0);
        assert_eq!(usage_percentage(50, 100), 50.0);
        assert_eq!(usage_percentage(150, 100), 100.0);
        assert_eq!(usage_percentage(100, 0), 0.0);
    }

    #[test]
    fn share_percent_matches_grok_context_formatting() {
        assert_eq!(share_percent(100, 0), "-");
        assert_eq!(share_percent(0, 0), "-");
        assert_eq!(share_percent(1, 1_000_000), "0.1%");
        assert_eq!(share_percent(0, 1_000_000), "0.0%");
        assert_eq!(share_percent(50_000, 1_000_000), "5.0%");
        assert_eq!(share_percent(500_000, 1_000_000), "50%");
        assert_eq!(share_percent(40, 100), "40%");
        assert_eq!(share_percent(150, 100), "100%");
    }

    #[test]
    fn group_thousands_groups_digits() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1,000");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn format_duration_buckets() {
        assert_eq!(format_duration(Duration::from_millis(500)), "0.5s");
        assert_eq!(format_duration(Duration::from_secs_f64(5.23)), "5.2s");
        assert_eq!(format_duration(Duration::from_secs(10)), "10s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m5s");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h2m");
    }
}

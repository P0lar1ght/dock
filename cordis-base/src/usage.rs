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

use std::collections::VecDeque;
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

/// 一次调用的费用来源。三态，缺费用与零费用永远分得开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCost {
    /// 上游在响应里带了费用（目前只有 xAI 的 `cost_in_usd_ticks`）。这是账单。
    Reported(i64),
    /// 按 `[model.<id>.pricing]` 本地算的。**不是账单**：单价可能过期，也不含
    /// 分时折扣（DeepSeek off-peak 半价），显示层要加"约"。
    Estimated(i64),
    /// 既没上报、也没配单价。**不是免费**。
    Unknown,
}

impl CallCost {
    /// 上游优先：报了就用报的，没报才拿 config 单价估。
    pub fn pick(reported: Option<i64>, estimated: Option<i64>) -> Self {
        match (reported_cost_ticks(reported), estimated.filter(|t| *t > 0)) {
            (Some(t), _) => Self::Reported(t),
            (None, Some(t)) => Self::Estimated(t),
            (None, None) => Self::Unknown,
        }
    }

    fn ticks(self) -> Option<i64> {
        match self {
            Self::Reported(t) | Self::Estimated(t) => Some(t),
            Self::Unknown => None,
        }
    }
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
    /// 既没上报、也没 config 单价可算的调用数。
    pub cost_missing_calls: u64,
    /// 用 `[model.<id>.pricing]` **本地估算**出来的调用数。
    ///
    /// 和上报的费用分开记：估算不是账单，单价过期、有分时折扣（DeepSeek
    /// off-peak 半价）时都会偏。显示层据此加"约"字，不能混成一个数就完事。
    pub cost_estimated_calls: u64,
}

/// `/usage` 保留多少次调用的明细。只够看出「命中率在哪一轮掉下去」这一件事，
/// 不是审计日志——留太多既占内存，画成 sparkline 也糊成一团。
pub const RECENT_CALLS_KEPT: usize = 40;

impl UsageTotals {
    /// 没命中缓存、按全价计费的输入。
    ///
    /// 三条 wire 归一到同一个约定：`input_tokens` 是**全量**输入，
    /// `cached_read` 与 `cache_creation` 是它互不相交的两个子集（Anthropic 的
    /// `input_tokens` 原本不含这两项，`messages.rs` 在解析时已经加了回去）。
    /// 所以这里是减法，不是别的口径。
    pub fn uncached_input_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cached_read_tokens)
            .saturating_sub(self.cache_creation_tokens)
    }

    /// 命中率。`None` = 这一格没有输入，不能拿 0% 冒充。
    pub fn cache_hit_rate(&self) -> Option<f64> {
        (self.input_tokens > 0).then(|| {
            self.cached_read_tokens.min(self.input_tokens) as f64 / self.input_tokens as f64
        })
    }

    fn from_call(usage: &TokenUsage, api_duration_ms: Option<u64>, cost: CallCost) -> Self {
        let cost_usd_ticks = cost.ticks();
        Self {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cached_read_tokens: usage.cached_prompt_tokens,
            cache_creation_tokens: usage.cache_creation_prompt_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            model_calls: 1,
            api_duration_ms: api_duration_ms.unwrap_or(0),
            cost_usd_ticks,
            cost_missing_calls: u64::from(cost == CallCost::Unknown),
            cost_estimated_calls: u64::from(matches!(cost, CallCost::Estimated(_))),
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
            cost_estimated_calls,
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
        self.cost_estimated_calls = self
            .cost_estimated_calls
            .saturating_add(*cost_estimated_calls);
        self.cost_usd_ticks = merge_cost_ticks(self.cost_usd_ticks, *cost_usd_ticks);
    }

    /// This total minus `earlier` (saturating per counter).
    fn saturating_sub(&self, earlier: &UsageTotals) -> UsageTotals {
        UsageTotals {
            input_tokens: self.input_tokens.saturating_sub(earlier.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(earlier.output_tokens),
            cached_read_tokens: self
                .cached_read_tokens
                .saturating_sub(earlier.cached_read_tokens),
            cache_creation_tokens: self
                .cache_creation_tokens
                .saturating_sub(earlier.cache_creation_tokens),
            reasoning_tokens: self
                .reasoning_tokens
                .saturating_sub(earlier.reasoning_tokens),
            model_calls: self.model_calls.saturating_sub(earlier.model_calls),
            api_duration_ms: self.api_duration_ms.saturating_sub(earlier.api_duration_ms),
            cost_usd_ticks: match (self.cost_usd_ticks, earlier.cost_usd_ticks) {
                (None, _) => None,
                (Some(now), None) => Some(now),
                (Some(now), Some(before)) => Some(now.saturating_sub(before)),
            },
            cost_missing_calls: self
                .cost_missing_calls
                .saturating_sub(earlier.cost_missing_calls),
            cost_estimated_calls: self
                .cost_estimated_calls
                .saturating_sub(earlier.cost_estimated_calls),
        }
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
    /// 子代理折进来的那部分（`totals` 里也算了一份，这里是它的子集）。
    ///
    /// 不单记的话，会话总计里「未命中」的大头到底是主循环每轮的新内容、还是
    /// 每个子代理各自的冷启动，就没有任何一处看得出来——而这两件事的处理方式
    /// 完全不同。
    pub subagent_totals: UsageTotals,
    /// 旁路调用：压缩这类既不进会话、也不是子代理的采样（`totals` 的子集）。
    pub side_totals: UsageTotals,
    /// Main-agent loop rounds for `num_turns` (subagents excluded).
    pub main_loop_model_calls: u64,
    /// Last main-loop call (for `/usage` 上一轮命中). Subagents do not overwrite.
    pub last_call: Option<UsageTotals>,
    /// 最近 [`RECENT_CALLS_KEPT`] 次主循环调用，新的在后。
    ///
    /// 只有一个 `last_call` 时看不出「这一轮新内容本来就多」和「前缀被打断了、
    /// 整段重算」的区别——两者都表现为命中率低。要区分只能看走势。
    ///
    /// `VecDeque` 而不是 `Vec`：语义就是「推新弹旧」的定长队列，`Vec::remove(0)`
    /// 每次调用都要 memmove 一遍。40 项时无所谓，但调大上限就成了隐患。
    pub recent_calls: VecDeque<UsageTotals>,
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
        cost: CallCost,
    ) {
        let call = UsageTotals::from_call(usage, api_duration_ms, cost);
        self.main_loop_model_calls = self.main_loop_model_calls.saturating_add(1);
        self.last_call = Some(call.clone());
        if self.recent_calls.len() == RECENT_CALLS_KEPT {
            self.recent_calls.pop_front();
        }
        self.recent_calls.push_back(call.clone());
        self.fold_entry(model_id, &call);
    }

    /// Fold subagent usage without incrementing `main_loop_model_calls`.
    pub fn record_subagent(&mut self, by_model: &[(String, UsageTotals)], incomplete: bool) {
        for (model_id, totals) in by_model {
            self.fold_entry(model_id, totals);
            self.subagent_totals.fold_totals(totals);
        }
        if incomplete {
            self.incomplete = true;
        }
    }

    /// 记一次**旁路**调用：压缩这类不进会话、也不算子代理的采样。
    ///
    /// 进 `totals` / `by_model`（它确实花了钱），但不碰 `main_loop_model_calls`
    /// 与 `recent_calls`——它不是用户的一轮，塞进走势图会把「每轮命中率」这条
    /// 轴的含义搅浑。压缩恰恰是命中率掉格最大的单次事件，不记的话 `/usage` 里
    /// 既看不到它的开销、也无从解释它之后那次全量重算。
    pub fn record_side_call(
        &mut self,
        model_id: &str,
        usage: &TokenUsage,
        api_duration_ms: Option<u64>,
        cost: CallCost,
    ) {
        let call = UsageTotals::from_call(usage, api_duration_ms, cost);
        self.side_totals.fold_totals(&call);
        self.fold_entry(model_id, &call);
    }

    /// Usage accumulated since `earlier`. A continuable child keeps one
    /// cumulative ledger across turns, so re-folding the whole thing every turn
    /// would bill the parent multiplicatively; the runner folds this delta
    /// instead. The per-model deltas are also accumulated into `totals`, so
    /// `fold_subagent_ledger`'s `totals.model_calls` guard sees real work.
    pub fn delta_since(&self, earlier: &UsageLedger) -> UsageLedger {
        let mut delta = UsageLedger {
            incomplete: self.incomplete && !earlier.incomplete,
            ..Default::default()
        };
        for (model_id, totals) in &self.by_model {
            let before = earlier.by_model.get(model_id).cloned().unwrap_or_default();
            let mut row = totals.saturating_sub(&before);
            // Equal reported ticks mean "no new cost", not "free": a real call
            // never reports Some(0) (zero cost is dropped at capture), so
            // normalize it away or an idle turn would keep a non-empty row.
            if row.cost_usd_ticks == Some(0) {
                row.cost_usd_ticks = None;
            }
            if row == UsageTotals::default() {
                continue;
            }
            delta.totals.fold_totals(&row);
            delta.by_model.insert(model_id.clone(), row);
        }
        delta
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
    /// 最近若干次主循环调用（旧 → 新），`/usage` 的命中率走势用。
    #[serde(default, rename = "recentCalls", skip_serializing_if = "Vec::is_empty")]
    pub recent_calls: Vec<PromptUsageModel>,
    /// 总计里属于子代理的那部分。`None` = 这次会话没有子代理。
    #[serde(
        default,
        rename = "subagentUsage",
        skip_serializing_if = "Option::is_none"
    )]
    pub subagent: Option<PromptUsageModel>,
    /// 总计里属于旁路调用（压缩）的那部分。`None` = 一次都没跑过。
    #[serde(default, rename = "sideUsage", skip_serializing_if = "Option::is_none")]
    pub side: Option<PromptUsageModel>,
}

impl PromptUsage {
    /// 「未命中」按来源拆开：主循环 / 子代理 / 压缩，非零的才出现。
    ///
    /// `None` = 只有主循环，拆了等于把同一个数写两遍。有子代理时这一行才是
    /// 会话总计与「上一轮」对得上的唯一途径：总计折了子代理和压缩，而上一轮 /
    /// 走势只有主循环，两个口径并排摆着最容易被读成「每轮都在漏 token」。
    pub fn miss_breakdown(&self) -> Option<Vec<(&'static str, u64)>> {
        let sub = self
            .subagent
            .as_ref()
            .map(PromptUsageModel::uncached_input_tokens);
        let side = self
            .side
            .as_ref()
            .map(PromptUsageModel::uncached_input_tokens);
        if sub.unwrap_or(0) == 0 && side.unwrap_or(0) == 0 {
            return None;
        }
        let total = self.totals.uncached_input_tokens();
        let main = total
            .saturating_sub(sub.unwrap_or(0))
            .saturating_sub(side.unwrap_or(0));
        let mut out = vec![("主循环", main)];
        if let Some(n) = sub.filter(|n| *n > 0) {
            out.push(("子代理", n));
        }
        if let Some(n) = side.filter(|n| *n > 0) {
            out.push(("压缩", n));
        }
        Some(out)
    }

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
    /// 其中有几次是本地估算的（见 [`CallCost::Estimated`]）。
    #[serde(default, skip_serializing)]
    pub cost_estimated_calls: u64,
}

/// 完整输入的一段。三段互不相交、加起来等于 `input_tokens`。
///
/// 口径、文案、「写入为 0 不占行」的规则只在这里定义一次：之前 spine 的文本块、
/// TUI 的数字表 / bar / 图例 / 按模型行各写了一遍，改一处就漏三处。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSegmentKind {
    /// 没命中缓存，按全价计费。Claude Code `/cost` 里的「input」就是这段。
    Miss,
    Hit,
    Write,
}

impl CacheSegmentKind {
    /// 数字表里的长标签。`Miss` 带括号是因为光写「输入」会和「完整输入」混。
    pub fn label(self) -> &'static str {
        match self {
            Self::Miss => "输入(未命中)",
            Self::Hit => "缓存命中",
            Self::Write => "缓存写入",
        }
    }

    /// bar 图例里的短标签，旁边就是色块，不需要再说一遍「输入」。
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Miss => "未命中",
            Self::Hit => "命中",
            Self::Write => "写入",
        }
    }

    /// 计价口径。三段单价不同，这正是要拆开显示的理由。
    pub fn note(self) -> &'static str {
        match self {
            Self::Miss => "全价计费",
            Self::Hit => "按缓存价计费",
            Self::Write => "首次落盘，比原价贵",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSegment {
    pub kind: CacheSegmentKind,
    pub tokens: u64,
}

impl PromptUsageModel {
    /// 见 [`UsageTotals::uncached_input_tokens`]——没命中、按全价计费的输入。
    pub fn uncached_input_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cached_read_tokens)
            .saturating_sub(self.cache_creation_tokens)
    }

    /// 命中率。`None` = 没有输入，不拿 0% 冒充。
    pub fn cache_hit_rate(&self) -> Option<f64> {
        (self.input_tokens > 0).then(|| {
            self.cached_read_tokens.min(self.input_tokens) as f64 / self.input_tokens as f64
        })
    }

    /// 完整输入拆成的三段，顺序固定（未命中 → 命中 → 写入），所有画法共用。
    ///
    /// **写入为 0 时整段不出现**：那多半是这条 wire 根本不报它
    /// （chat/completions、Responses），不是"真的没写"——列一行 0 会被读成缓存
    /// 没生效。未命中与命中即使为 0 也保留，它们每条 wire 都报。
    pub fn cache_segments(&self) -> Vec<CacheSegment> {
        let mut out = vec![
            CacheSegment {
                kind: CacheSegmentKind::Miss,
                tokens: self.uncached_input_tokens(),
            },
            CacheSegment {
                kind: CacheSegmentKind::Hit,
                tokens: self.cached_read_tokens,
            },
        ];
        if self.cache_creation_tokens > 0 {
            out.push(CacheSegment {
                kind: CacheSegmentKind::Write,
                tokens: self.cache_creation_tokens,
            });
        }
        out
    }

    /// 该段占完整输入的比例，格式同 `/context`。
    pub fn segment_share(&self, segment: &CacheSegment) -> String {
        share_percent(segment.tokens, self.input_tokens)
    }

    /// 该段的计价口径。比 [`CacheSegmentKind::note`] 多知道一件事：这条 wire
    /// 报不报「写入」。
    ///
    /// chat/completions 与 Responses 不单独上报 `cache_creation`，于是**这一轮
    /// 新写进缓存的 token 全落在「未命中」里**。不写明的话，这个数和 Claude
    /// Code `/cost` 的 `input` 看着是同一个口径，其实差着一整个桶——那边把它
    /// 单列成 cache write 了。
    pub fn segment_note(&self, segment: &CacheSegment) -> String {
        let base = segment.kind.note();
        if segment.kind == CacheSegmentKind::Miss
            && self.cache_creation_tokens == 0
            && self.cached_read_tokens > 0
        {
            return format!("{base} · 含首次写入（这条 wire 不单列）");
        }
        base.to_string()
    }
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
            cost_estimated_calls,
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
            cost_estimated_calls,
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
            recent_calls: ledger
                .recent_calls
                .iter()
                .map(PromptUsageModel::from)
                .collect(),
            subagent: (ledger.subagent_totals != UsageTotals::default())
                .then(|| PromptUsageModel::from(&ledger.subagent_totals)),
            side: (ledger.side_totals != UsageTotals::default())
                .then(|| PromptUsageModel::from(&ledger.side_totals)),
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

    // Claude Code `/cost` 的口径：**「输入」= 未命中的那部分**，缓存读 / 写各自
    // 单列。分段与文案见 [`PromptUsageModel::cache_segments`]。
    let mut rows: Vec<String> = t
        .cache_segments()
        .iter()
        .map(|seg| {
            format!(
                "  {}{} · {}",
                pad_label(&format!("{}:", seg.kind.label()), 15),
                group_thousands(seg.tokens),
                t.segment_share(seg),
            )
        })
        .collect();
    rows.push(format!(
        "  {}{}（思考 {}）",
        pad_label("输出:", 15),
        group_thousands(t.output_tokens),
        group_thousands(t.reasoning_tokens),
    ));
    rows.push(format!(
        "  {}{} · 合计 {}",
        pad_label("完整输入:", 15),
        group_thousands(t.input_tokens),
        group_thousands(t.total_tokens),
    ));
    if let Some(last) = &usage.last_call {
        rows.push(format!(
            "  上一轮:        输入(未命中) {} · 缓存命中 {} · 完整输入 {} · {}",
            group_thousands(last.uncached_input_tokens()),
            group_thousands(last.cached_read_tokens),
            group_thousands(last.input_tokens),
            share_percent(last.cached_read_tokens, last.input_tokens),
        ));
    }
    if let Some(breakdown) = miss_breakdown_text(usage) {
        rows.push(format!("  {}{breakdown}", pad_label("未命中来源:", 15)));
    }
    if let Some(trend) = hit_rate_trend(&usage.recent_calls, RECENT_CALLS_KEPT) {
        rows.push(format!(
            "  每轮命中率:    {trend}（旧 → 新 · 最近 {} 次主循环调用）",
            trend.chars().count(),
        ));
    }
    rows.push(format!(
        "  模型调用:      {} · API 耗时: {}",
        calls_breakdown(usage),
        format_duration(Duration::from_millis(t.api_duration_ms)),
    ));
    rows.push(format!("  费用:          {}", format_cost(t)));

    if usage.model_usage.len() > 1 {
        rows.push("  按模型:".to_string());
        for (model, m) in &usage.model_usage {
            rows.push(format!(
                "    {model}: {} · {}",
                per_model_amounts(m),
                format_cost(m)
            ));
        }
    }

    if usage.usage_is_incomplete {
        rows.push("  说明：用量统计不完整，可能少计。".to_string());
    }

    join_header_rows("本会话用量（自开始或上次恢复）：".to_string(), rows)
}

/// 调用次数怎么念。
///
/// `totals.model_calls` 折进了子代理（`record_subagent` 也走 `fold_entry`），
/// 而走势图只有主循环（`recent_calls` 只由 `record_main_loop_call` 写）。两个数
/// 并排摆着又不说明来历，就会读成「走势少画了一格」——所以差额必须写出来。
pub fn calls_breakdown(usage: &PromptUsage) -> String {
    let total = usage.totals.model_calls;
    let side = usage.side.as_ref().map(|s| s.model_calls).unwrap_or(0);
    // 有分账就用分账。没有（旧账本 / 手搓的 payload）才退回减法，那时差额一律
    // 算子代理——以前压缩根本不记账，减法从来没见过它。
    let sub = usage
        .subagent
        .as_ref()
        .map(|s| s.model_calls)
        .unwrap_or_else(|| total.saturating_sub(usage.num_turns).saturating_sub(side));
    if sub == 0 && side == 0 {
        return group_thousands(total);
    }
    let mut parts = vec![format!("主循环 {}", group_thousands(usage.num_turns))];
    if sub > 0 {
        parts.push(format!("子代理 {}", group_thousands(sub)));
    }
    if side > 0 {
        parts.push(format!("压缩 {}", group_thousands(side)));
    }
    format!("{}（{}）", group_thousands(total), parts.join(" · "))
}

/// 「未命中」按来源拆成一行，见 [`PromptUsage::miss_breakdown`]。
pub fn miss_breakdown_text(usage: &PromptUsage) -> Option<String> {
    Some(
        usage
            .miss_breakdown()?
            .into_iter()
            .map(|(label, tokens)| format!("{label} {}", group_thousands(tokens)))
            .collect::<Vec<_>>()
            .join(" \u{00b7} "),
    )
}

/// Sparkline 的八级方块，低 → 高。
const SPARK_LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// 一次调用的命中率 → 一个方块。刻度固定成 0..100%，不按样本自适应：自适应
/// 的话一段全是 90% 上下的调用会被拉成大起大落，看起来像出了问题。
///
/// 有输入但一点没命中时给最低的一格而不是空白——空白读起来像「没有这次调用」。
pub fn hit_rate_spark(call: &PromptUsageModel) -> char {
    let Some(rate) = call.cache_hit_rate() else {
        return ' ';
    };
    let idx = (rate * SPARK_LEVELS.len() as f64).floor() as usize;
    SPARK_LEVELS[idx.min(SPARK_LEVELS.len() - 1)]
}

/// 整段走势，最多 `max_cells` 格。`None` = 不足两次调用，一个格子看不出走势。
///
/// 放不下时**丢最旧的、留最新的**：数据是旧 → 新排的，交给渲染层在右边裁等于
/// 把刚发生的那几次裁掉，正好是最该看的。
///
/// 没有输入的调用不占格子（[`hit_rate_spark`] 对它返回空格，两处守卫必须同时
/// 在：这里滤掉，spark 那边兜底）。所以格数是「有输入的主循环调用数」，可能
/// 少于 `num_turns`——调用方要把**实际格数**报给用户，别拿别的计数去凑。
pub fn hit_rate_trend(calls: &[PromptUsageModel], max_cells: usize) -> Option<String> {
    let mut rendered: Vec<char> = calls
        .iter()
        .filter(|c| c.input_tokens > 0)
        .map(hit_rate_spark)
        .collect();
    if rendered.len() > max_cells {
        rendered.drain(..rendered.len() - max_cells);
    }
    (rendered.len() >= 2).then(|| rendered.into_iter().collect())
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

/// 按模型一行：同样是 Claude Code 的口径（输入 = 未命中）。文本块与 overlay
/// 共用，两边各写一遍正是这次要收掉的东西。
pub fn per_model_amounts(m: &PromptUsageModel) -> String {
    m.cache_segments()
        .iter()
        .map(|seg| format!("{} {}", group_thousands(seg.tokens), seg.kind.label()))
        .chain(std::iter::once(format!(
            "{} 输出",
            group_thousands(m.output_tokens)
        )))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 左对齐到指定显示宽度。中文字符宽 2，不能按 `char` 数补。
///
/// 这里的标签是一组固定常量，只有 ASCII 与 CJK 两种宽度，所以自己数就够；
/// 为它们给 spine 拉一个 `unicode-width` 依赖不划算（TUI 那边本来就有）。
fn pad_label(label: &str, width: usize) -> String {
    let w: usize = label
        .chars()
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum();
    format!("{label}{}", " ".repeat(width.saturating_sub(w)))
}

/// Copied from grok pager `percent_of_window`. Tiny nonzero shares floor at
/// `0.1%`; `part` is clamped to `total` so a cache subset cannot read over 100%.
///
/// `/context` 的占比和 `/usage` 的缓存占比是同一条规则，只此一份。
pub fn share_percent(part: u64, total: u64) -> String {
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

/// 费用文案。
///
/// 三种来源永不混成一个说法：
/// - 全部来自上游 → `$X`，这是账单。
/// - 掺了本地估算 → 加"约"并说明来源。单价可能过期，也不含分时折扣
///   （DeepSeek off-peak 半价），把它当账单读会亏。
/// - 有调用连价都算不出 → 沿用既有的 partial 措辞。`None` 一律是"未上报"，
///   **不是免费**（Grok fail-closed）。
pub fn format_cost(m: &PromptUsageModel) -> String {
    let Some(ticks) = m.cost_usd_ticks else {
        return if m.cost_is_partial {
            "未上报（部分调用无费用）".to_string()
        } else {
            "未上报".to_string()
        };
    };
    let amount = format!("${:.4}", ticks_to_usd(ticks));
    let reported = m
        .model_calls
        .saturating_sub(m.cost_estimated_calls)
        .saturating_sub(m.cost_missing_calls);
    match (m.cost_estimated_calls, m.cost_missing_calls, reported) {
        (0, 0, _) => amount,
        (0, _, _) => format!("约 {amount}（部分调用无费用）"),
        (_, 0, 0) => format!("约 {amount}（按 config 单价估算）"),
        (_, 0, _) => format!("约 {amount}（部分上报 · 部分估算）"),
        _ => format!("约 {amount}（部分估算 · 部分调用无费用）"),
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
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
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
            cost_estimated_calls: 0,
        }
    }

    #[test]
    fn delta_since_totals_and_by_model_match_a_single_fold() {
        let mut cumulative = UsageLedger::default();
        cumulative.record_main_loop_call("a", &tu(100, 10), Some(50), CallCost::Reported(70));
        cumulative.record_main_loop_call("b", &tu(30, 5), Some(20), CallCost::Unknown);

        let delta = cumulative.delta_since(&UsageLedger::default());

        // The delta carries only usage, not main-loop bookkeeping.
        assert_eq!(delta.main_loop_model_calls, 0);
        assert_eq!(delta.last_call, None);
        assert_eq!(delta.totals, cumulative.totals);
        assert_eq!(delta.by_model, cumulative.by_model);
        assert_eq!(delta.totals.model_calls, 2);
        assert_eq!(delta.totals.cost_usd_ticks, Some(70));
        assert!(delta.totals.cost_is_partial());
        assert_eq!(delta.by_model["a"].input_tokens, 100);
        assert_eq!(delta.by_model["b"].input_tokens, 30);
        assert!(!delta.incomplete);
    }

    #[test]
    fn delta_since_second_delta_only_carries_new_calls() {
        let mut cumulative = UsageLedger::default();
        cumulative.record_main_loop_call("a", &tu(100, 10), Some(50), CallCost::Reported(70));
        let first = cumulative.delta_since(&UsageLedger::default());

        cumulative.record_main_loop_call("a", &tu(40, 4), Some(10), CallCost::Reported(30));
        let second = cumulative.delta_since(&first);

        assert_eq!(second.totals.model_calls, 1);
        assert_eq!(second.totals.input_tokens, 40);
        assert_eq!(second.totals.output_tokens, 4);
        assert_eq!(second.totals.cost_usd_ticks, Some(30));
        assert_eq!(second.by_model.len(), 1);
        assert_eq!(second.by_model["a"].input_tokens, 40);
    }

    #[test]
    fn delta_since_flags_newly_incomplete_only() {
        let mut cumulative = UsageLedger::default();
        cumulative.record_main_loop_call("a", &tu(10, 1), None, CallCost::Unknown);
        cumulative.mark_incomplete();
        assert!(cumulative.delta_since(&UsageLedger::default()).incomplete);

        let earlier = cumulative.clone();
        assert!(!cumulative.delta_since(&earlier).incomplete);
    }

    /// Mirrors the runner loop: fold cumulative-ledger deltas turn-by-turn into
    /// a parent ledger; the parent must end exactly at the child's final total
    /// (no double counting, no under counting), and the `mark_incomplete`
    /// cancel path must survive the fold.
    #[test]
    fn turn_by_turn_delta_folding_matches_child_final_total() {
        let mut child = UsageLedger::default();
        let mut folded = UsageLedger::default();
        let mut parent = UsageLedger::default();

        // Turn 1: one call on model a.
        child.record_main_loop_call("a", &tu(100, 10), Some(50), CallCost::Reported(70));
        let delta = child.delta_since(&folded);
        assert!(delta.totals.model_calls > 0);
        parent.record_subagent(
            &delta
                .by_model
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            delta.incomplete,
        );
        folded = child.clone();

        // Turn 2: another call on the same model plus a new model.
        child.record_main_loop_call("a", &tu(40, 4), Some(10), CallCost::Reported(30));
        child.record_main_loop_call("b", &tu(20, 2), Some(5), CallCost::Unknown);
        let delta = child.delta_since(&folded);
        assert!(delta.totals.model_calls > 0);
        parent.record_subagent(
            &delta
                .by_model
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            delta.incomplete,
        );
        folded = child.clone();

        // Turn 3: no usage at all — an empty delta must be a no-op, or idle
        // parking would bill the parent again.
        let delta = child.delta_since(&folded);
        assert_eq!(delta.totals, UsageTotals::default());
        assert!(delta.by_model.is_empty());
        parent.record_subagent(
            &delta
                .by_model
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            delta.incomplete,
        );
        folded = child.clone();

        // Turn 4: one call, then cancelled mid-turn — runner marks the delta
        // incomplete and the parent must inherit the flag.
        child.record_main_loop_call("a", &tu(30, 3), Some(5), CallCost::Unknown);
        let mut delta = child.delta_since(&folded);
        assert!(delta.totals.model_calls > 0);
        delta.mark_incomplete();
        parent.record_subagent(
            &delta
                .by_model
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            delta.incomplete,
        );

        assert_eq!(
            parent.totals, child.totals,
            "parent must equal child exactly"
        );
        assert_eq!(parent.by_model, child.by_model);
        assert_eq!(parent.totals.model_calls, 4);
        assert_eq!(parent.totals.input_tokens, 100 + 40 + 20 + 30);
        assert_eq!(parent.totals.cost_usd_ticks, Some(70 + 30));
        assert!(parent.incomplete);
        assert_eq!(
            parent.main_loop_model_calls, 0,
            "subagent calls are not turns"
        );
    }

    #[test]
    fn ledger_sums_partial_subagent_and_zero_cost() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("m", &tu(1, 1), None, CallCost::Unknown);
        assert_eq!(ledger.totals.cost_usd_ticks, None);
        assert_eq!(ledger.totals.cost_missing_calls, 1);

        ledger.record_main_loop_call("a", &tu(100, 10), Some(100), CallCost::Unknown);
        ledger.record_main_loop_call("a", &tu(50, 5), Some(50), CallCost::Reported(70));
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
        ledger.record_main_loop_call(
            "grok-build",
            &call,
            Some(50),
            CallCost::Reported(20_000_000),
        );
        let v = serde_json::to_value(PromptUsage::from(&ledger)).unwrap();
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
        ledger.record_main_loop_call("a", &tu(100, 10), None, CallCost::Reported(70));
        ledger.record_main_loop_call("a", &tu(50, 5), None, CallCost::Unknown);
        let v = serde_json::to_value(PromptUsage::from(&ledger)).unwrap();
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
            text.contains(
                "上一轮:        输入(未命中) 1,000 · 缓存命中 12,000 · 完整输入 13,000 · 92%"
            ),
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
        assert!(
            text.contains("完整输入:      1,234,567 · 合计 1,280,245"),
            "{text}"
        );
        assert!(text.contains("缓存命中:      1,000,000 · 81%"), "{text}");
        // Claude Code 口径：「输入」就是未命中那部分，要能自己算出来。
        assert!(text.contains("输入(未命中):  234,567 · 19%"), "{text}");
        assert!(!text.contains("缓存写入"), "没有写入就不占一行：{text}");
        assert!(!text.contains("上一轮命中"), "{text}");
        assert!(text.contains("思考 12,000"), "{text}");
        assert!(text.contains("$1.2345"), "{text}");
        assert!(text.contains("3m12s"), "{text}");
        assert!(!text.contains("按模型"), "{text}");
    }

    /// 三条 wire 归一后，输入 = 命中 + 写入 + 未命中，减法才成立。
    /// Anthropic 的 `input_tokens` 本来不含另外两项，`messages.rs` 解析时补过。
    #[test]
    fn uncached_input_is_the_rest_of_the_prompt() {
        let mut row = model_row(1_000, 10, None);
        row.cached_read_tokens = 600;
        row.cache_creation_tokens = 150;
        assert_eq!(row.uncached_input_tokens(), 250);
        assert_eq!(row.cache_hit_rate(), Some(0.6));

        // 上游只报了读、没报写：剩下的全算未命中，不能变成负数。
        let mut read_only = model_row(1_000, 10, None);
        read_only.cached_read_tokens = 1_200;
        assert_eq!(read_only.uncached_input_tokens(), 0);
        assert_eq!(read_only.cache_hit_rate(), Some(1.0), "命中率封顶 100%");

        // 没有输入不能拿 0% 冒充「一点没命中」。
        assert_eq!(model_row(0, 0, None).cache_hit_rate(), None);
    }

    /// 上游优先：报了就用报的；没报才拿 config 单价估；都没有是 `Unknown`
    /// （**不是免费**）。上报的 0 / 负数在 `reported_cost_ticks` 就被判成没上报。
    #[test]
    fn call_cost_prefers_the_upstream_bill_over_the_local_estimate() {
        assert_eq!(
            CallCost::pick(Some(700), Some(999)),
            CallCost::Reported(700)
        );
        assert_eq!(CallCost::pick(None, Some(999)), CallCost::Estimated(999));
        assert_eq!(CallCost::pick(None, None), CallCost::Unknown);
        assert_eq!(
            CallCost::pick(Some(0), Some(999)),
            CallCost::Estimated(999),
            "上游报 0 = 没上报，不是免费"
        );
        assert_eq!(
            CallCost::pick(None, Some(0)),
            CallCost::Unknown,
            "算出来 0 说明没配单价"
        );
    }

    /// 估算永远带"约"，且不同来源的措辞分得开——把估算读成账单会亏。
    #[test]
    fn cost_label_distinguishes_bill_from_estimate() {
        let row = |calls: u64, estimated: u64, missing: u64| PromptUsageModel {
            model_calls: calls,
            cost_usd_ticks: Some(12_345_000_000),
            cost_estimated_calls: estimated,
            cost_missing_calls: missing,
            ..Default::default()
        };
        assert_eq!(format_cost(&row(3, 0, 0)), "$1.2345");
        assert_eq!(
            format_cost(&row(3, 3, 0)),
            "约 $1.2345（按 config 单价估算）"
        );
        assert_eq!(
            format_cost(&row(3, 1, 0)),
            "约 $1.2345（部分上报 · 部分估算）"
        );
        assert_eq!(
            format_cost(&row(3, 1, 1)),
            "约 $1.2345（部分估算 · 部分调用无费用）"
        );

        // 一分钱都算不出时不能显示 $0。
        let none = PromptUsageModel {
            model_calls: 2,
            cost_usd_ticks: None,
            cost_missing_calls: 2,
            ..Default::default()
        };
        assert_eq!(format_cost(&none), "未上报");
    }

    /// 估算的调用数要能一路折到会话总计，否则长会话里"约"字会掉。
    #[test]
    fn estimated_calls_fold_into_the_session_total() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("m", &tu(100, 10), None, CallCost::Estimated(500));
        ledger.record_main_loop_call("m", &tu(100, 10), None, CallCost::Reported(700));
        let usage = PromptUsage::from(&ledger);
        assert_eq!(usage.totals.cost_usd_ticks, Some(1_200));
        assert_eq!(usage.totals.cost_estimated_calls, 1);
        assert_eq!(usage.totals.cost_missing_calls, 0);
        assert!(
            format_cost(&usage.totals).contains("部分上报 · 部分估算"),
            "{}",
            format_cost(&usage.totals)
        );
    }

    /// 三段是完整输入的一个划分：互不相交、加起来正好等于 `input_tokens`。
    /// 所有画法都由它派生，这条不成立的话 bar、数字表、按模型行会一起错。
    #[test]
    fn cache_segments_partition_the_full_input() {
        let mut row = model_row(1_000, 10, None);
        row.cached_read_tokens = 600;
        row.cache_creation_tokens = 150;
        let segs = row.cache_segments();
        assert_eq!(
            segs.iter().map(|s| s.tokens).sum::<u64>(),
            row.input_tokens,
            "三段之和必须等于完整输入"
        );
        assert_eq!(
            segs.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![
                CacheSegmentKind::Miss,
                CacheSegmentKind::Hit,
                CacheSegmentKind::Write
            ],
            "顺序固定，bar 与图例才对得上"
        );
        assert_eq!(row.segment_share(&segs[0]), "25%");

        // 不报写入的 wire：只有两段，且仍然是一个划分。
        row.cache_creation_tokens = 0;
        let segs = row.cache_segments();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs.iter().map(|s| s.tokens).sum::<u64>(), row.input_tokens);
    }

    /// 写入有量才占一行——没有写入的 wire（chat/completions、Responses）不该
    /// 平白多出一行 0。
    #[test]
    fn cache_write_row_only_when_the_wire_reports_writes() {
        let mut totals = model_row(1_000, 10, None);
        totals.cached_read_tokens = 400;
        totals.cache_creation_tokens = 100;
        let text = session_usage_block_text(&PromptUsage {
            totals,
            ..Default::default()
        });
        assert!(text.contains("缓存写入:      100 · 10%"), "{text}");
        assert!(text.contains("输入(未命中):  500 · 50%"), "{text}");
        assert!(text.contains("缓存命中:      400 · 40%"), "{text}");
    }

    /// Sparkline 的刻度是固定的 0..100%，不按样本自适应：自适应会把一串都在
    /// 90% 上下的调用画成大起大落，看起来像出了问题。
    #[test]
    fn hit_rate_spark_uses_an_absolute_scale() {
        let spark = |input: u64, cached: u64| {
            let mut row = model_row(input, 0, None);
            row.cached_read_tokens = cached;
            hit_rate_spark(&row)
        };
        assert_eq!(spark(100, 0), '▁', "有输入但零命中要占最低一格，不是空白");
        assert_eq!(spark(100, 100), '█');
        assert_eq!(spark(100, 50), '▅');
        // 都在高位的样本必须画得一样高，不能被拉伸。
        assert_eq!(spark(100, 90), spark(100, 95));
        assert_eq!(spark(0, 0), ' ', "没有输入 = 没有这一格");
    }

    /// 一次调用画不出走势，不如不画。
    #[test]
    fn hit_rate_trend_needs_at_least_two_calls() {
        let one = [model_row(100, 1, None)];
        assert_eq!(hit_rate_trend(&one, 40), None);

        let mut hot = model_row(100, 1, None);
        hot.cached_read_tokens = 100;
        let calls = [model_row(100, 1, None), hot, model_row(0, 0, None)];
        assert_eq!(
            hit_rate_trend(&calls, 40).as_deref(),
            Some("▁█"),
            "没有输入的调用不占格子"
        );
    }

    /// 放不下时丢**最旧**的。数据是旧 → 新排的，交给渲染层在右边裁等于把刚
    /// 发生的那几次裁掉，正好是最该看的。
    #[test]
    fn hit_rate_trend_drops_the_oldest_not_the_newest() {
        let calls: Vec<PromptUsageModel> = (0..8)
            .map(|i| {
                let mut row = model_row(100, 1, None);
                // 命中率 0%,12%,25%…，每一格都不同，方向错了就看得出来。
                row.cached_read_tokens = i * 12;
                row
            })
            .collect();
        let full = hit_rate_trend(&calls, 40).unwrap();
        assert_eq!(full.chars().count(), 8);

        let clipped = hit_rate_trend(&calls, 3).unwrap();
        assert_eq!(clipped.chars().count(), 3);
        assert!(
            full.ends_with(&clipped),
            "留下的必须是最新的三格：full={full} clipped={clipped}"
        );
    }

    /// 会话总计折了子代理与压缩，「上一轮」和走势却只有主循环。不把未命中按
    /// 来源拆开，这两个口径并排摆着就会被读成「主循环每轮都在漏 token」——而
    /// 大头往往是子代理冷启动：各自一套系统提示，第一次调用必然整份满价。
    #[test]
    fn miss_breakdown_separates_subagent_cold_starts_from_the_main_loop() {
        let mut ledger = UsageLedger::default();
        // 主循环：每轮只差一条新消息没命中。
        for _ in 0..10 {
            let mut call = tu(100_000, 500);
            call.cached_prompt_tokens = 99_800;
            ledger.record_main_loop_call("m", &call, None, CallCost::Unknown);
        }
        // 子代理：一次冷启动，整份 prompt 全价。
        ledger.record_subagent(
            &[(
                "m".into(),
                UsageTotals {
                    input_tokens: 20_000,
                    output_tokens: 300,
                    cached_read_tokens: 0,
                    model_calls: 1,
                    ..Default::default()
                },
            )],
            false,
        );
        // 压缩：整段历史、几乎零命中。
        ledger.record_side_call("m", &tu(90_000, 900), None, CallCost::Unknown);

        let usage = PromptUsage::from(&ledger);
        assert_eq!(
            usage.totals.uncached_input_tokens(),
            2_000 + 20_000 + 90_000
        );
        assert_eq!(
            usage.miss_breakdown(),
            Some(vec![
                ("主循环", 2_000),
                ("子代理", 20_000),
                ("压缩", 90_000)
            ]),
            "未命中的来源分不开，总计就只能被读成主循环在漏"
        );
        let text = session_usage_block_text(&usage);
        assert!(
            text.contains("未命中来源:    主循环 2,000 · 子代理 20,000 · 压缩 90,000"),
            "{text}"
        );
        assert!(
            text.contains("模型调用:      12（主循环 10 · 子代理 1 · 压缩 1）"),
            "{text}"
        );

        // 只有主循环时拆开等于把同一个数写两遍。
        let mut plain = UsageLedger::default();
        plain.record_main_loop_call("m", &tu(100, 10), None, CallCost::Unknown);
        assert_eq!(PromptUsage::from(&plain).miss_breakdown(), None);
        assert!(!session_usage_block_text(&PromptUsage::from(&plain)).contains("未命中来源"));
    }

    /// 旁路调用（压缩）花的钱要进总计，但它不是用户的一轮：不进 `num_turns`，
    /// 也不占「每轮命中率」的格子。
    #[test]
    fn side_calls_are_billed_but_are_not_turns() {
        let mut ledger = UsageLedger::default();
        ledger.record_main_loop_call("m", &tu(1_000, 10), Some(100), CallCost::Reported(50));
        ledger.record_side_call("m", &tu(90_000, 900), Some(2_000), CallCost::Reported(700));

        assert_eq!(ledger.main_loop_model_calls, 1, "压缩不是一轮");
        assert_eq!(ledger.recent_calls.len(), 1, "压缩不进走势");
        assert_eq!(ledger.totals.input_tokens, 91_000);
        assert_eq!(ledger.totals.model_calls, 2);
        assert_eq!(ledger.side_totals.input_tokens, 90_000);
        assert_eq!(ledger.totals.cost_usd_ticks, Some(750), "压缩的钱也是钱");
        assert_eq!(ledger.by_model["m"].input_tokens, 91_000);
    }

    /// 不报写入的 wire（chat/completions、Responses）上，这一轮**新写进缓存**的
    /// token 全落在「未命中」里。不写明就会被当成和 Claude Code `/cost` 的
    /// `input` 同一个口径，而那边把它单列成了 cache write。
    #[test]
    fn miss_note_says_when_the_wire_folds_writes_into_it() {
        let mut no_write_bucket = model_row(1_000, 10, None);
        no_write_bucket.cached_read_tokens = 900;
        let segs = no_write_bucket.cache_segments();
        assert!(
            no_write_bucket
                .segment_note(&segs[0])
                .contains("含首次写入"),
            "{}",
            no_write_bucket.segment_note(&segs[0])
        );

        // 报写入的 wire（Messages）：写入自己占一段，未命中就是纯未命中。
        let mut with_bucket = model_row(1_000, 10, None);
        with_bucket.cached_read_tokens = 600;
        with_bucket.cache_creation_tokens = 300;
        let segs = with_bucket.cache_segments();
        assert_eq!(with_bucket.segment_note(&segs[0]), "全价计费");

        // 一次都没命中过（首轮）不算「这条 wire 不报写入」的证据。
        let cold = model_row(1_000, 10, None);
        assert_eq!(cold.segment_note(&cold.cache_segments()[0]), "全价计费");
    }

    /// 走势只画主循环，`model_calls` 还折了子代理进去。两个数并排摆着又不说明
    /// 来历，就会被读成「走势少画了一格」——差额必须写出来。
    #[test]
    fn call_counts_reconcile_with_the_trend() {
        let mut ledger = UsageLedger::default();
        for _ in 0..19 {
            let mut call = tu(1_000, 10);
            call.cached_prompt_tokens = 900;
            ledger.record_main_loop_call("m", &call, None, CallCost::Unknown);
        }
        let only_main = PromptUsage::from(&ledger);
        assert_eq!(
            calls_breakdown(&only_main),
            "19",
            "没有子代理时不该多一对括号"
        );

        ledger.record_subagent(
            &[(
                "sub".into(),
                UsageTotals {
                    input_tokens: 500,
                    model_calls: 1,
                    ..Default::default()
                },
            )],
            false,
        );
        let usage = PromptUsage::from(&ledger);
        let cells = hit_rate_trend(&usage.recent_calls, RECENT_CALLS_KEPT)
            .map(|t| t.chars().count())
            .unwrap_or(0);
        assert_eq!(usage.totals.model_calls, 20);
        assert_eq!(cells, 19, "子代理不进走势");
        assert_eq!(
            calls_breakdown(&usage),
            "20（主循环 19 · 子代理 1）",
            "差额要写出来，否则 20 与 19 格对不上"
        );
        assert!(session_usage_block_text(&usage).contains("最近 19 次主循环调用"));
    }

    /// 每次主循环调用都进历史，且有上限——长会话不能无限增长。
    #[test]
    fn recent_calls_are_bounded_and_ordered_oldest_first() {
        let mut ledger = UsageLedger::default();
        for i in 1..=(RECENT_CALLS_KEPT + 5) {
            ledger.record_main_loop_call("a", &tu(i as u64, 1), None, CallCost::Unknown);
        }
        assert_eq!(ledger.recent_calls.len(), RECENT_CALLS_KEPT);
        assert_eq!(
            ledger.recent_calls.front().unwrap().input_tokens,
            6,
            "最旧的几次被挤掉"
        );
        assert_eq!(
            ledger.recent_calls.back().unwrap().input_tokens,
            (RECENT_CALLS_KEPT + 5) as u64,
            "最新的在末尾"
        );

        // 子代理不是主循环的一轮，不进走势图。
        ledger.record_subagent(
            &[(
                "b".into(),
                UsageTotals {
                    input_tokens: 9_999,
                    model_calls: 1,
                    ..Default::default()
                },
            )],
            false,
        );
        assert_eq!(ledger.recent_calls.len(), RECENT_CALLS_KEPT);
        assert!(ledger.recent_calls.iter().all(|c| c.input_tokens != 9_999));
    }

    #[test]
    fn session_usage_block_lists_models_when_multiple() {
        let mut usage = PromptUsage {
            totals: model_row(150, 15, None),
            num_turns: 1,
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
        assert!(
            text.contains("grok-build: 100 输入(未命中) · 0 缓存命中 · 10 输出"),
            "{text}"
        );
        assert!(
            text.contains("grok-4: 50 输入(未命中) · 0 缓存命中 · 5 输出"),
            "{text}"
        );
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

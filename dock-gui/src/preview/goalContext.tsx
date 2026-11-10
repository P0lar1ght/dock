// `#goal-context`：目标条与上下文圆环的各个状态（假数据），对照 wonder-ui/chat/ComposerGoalContext。

import { useState } from "react";
import { Composer } from "../components/Composer";
import { ContextRing } from "../components/ContextRing";
import { GoalBar } from "../components/GoalBar";
import type { ComposerSettings, ContextBreakdown, GoalInfo } from "../model";

const TITLE = "把 gateway 的单测补到覆盖 settings 全部方法，跑绿为止";

const BREAKDOWN: ContextBreakdown = {
  model: "deepseek-flash",
  used: 58_900,
  total: 128_000,
  percent: 46,
  triggerPercent: 85,
  triggerTokens: 108_800,
  compactions: 1,
  slices: [
    {
      id: "system",
      label: "系统提示",
      group: "prefix",
      tokens: 3_900,
      note: null,
      includes: [
        { label: "技能", tokens: 800, note: "14 个 · 已计入系统提示" },
        { label: "工作流", tokens: 200, note: "2 个 · 已计入系统提示" },
      ],
      detail: [
        { heading: "3900 token · 9120 字 · 180 行", rows: [{ label: "基座", tokens: 2_100, note: null }, { label: "技能", tokens: 800, note: null }] },
      ],
    },
    {
      id: "tools",
      label: "工具定义",
      group: "prefix",
      tokens: 6_100,
      note: "38 个",
      includes: [],
      detail: [
        {
          heading: "38 个工具（模型可见，不含按需工具）",
          rows: [
            { label: "task", tokens: 1_400, note: "Launch a subagent…" },
            { label: "bash", tokens: 1_100, note: "Run a shell command…" },
            { label: "edit_file", tokens: 900, note: null },
            { label: "read_file", tokens: 600, note: null },
          ],
        },
      ],
    },
    {
      id: "messages",
      label: "消息",
      group: "session",
      tokens: 45_400,
      note: "12 轮 · 48 次工具调用",
      includes: [
        { label: "工程规约", tokens: 900, note: "AGENTS.md · 已计入消息" },
        { label: "记忆", tokens: 500, note: "MEMORY.md · 已计入消息" },
      ],
      detail: [{ heading: "用户 12 · 助手 60 · 工具结果 48 · 提醒 3", rows: [{ label: "工具结果", tokens: 30_100, note: "48 条" }] }],
    },
    {
      id: "overhead",
      label: "推理/开销",
      group: "session",
      tokens: 3_500,
      note: null,
      includes: [],
      detail: [{ heading: "", rows: [{ label: "推理", tokens: 3_500, note: "9 段" }] }],
    },
    { id: "free", label: "空闲", group: "free", tokens: 69_100, note: null, includes: [], detail: [] },
  ],
  onDemand: [{ id: "mcp", label: "MCP 服务器", note: "2 台 · 14 个工具 · 未计入窗口", detail: [] }],
};

const SETTINGS: ComposerSettings = {
  approval: "ask",
  model: "deepseek-flash",
  modelId: "deepseek-flash",
  models: [{ id: "deepseek-flash", label: "deepseek-flash" }],
  reasoning: "",
  reasoningOptions: [],
  plan: false,
  goal: null,
  context: { used: 58_900, total: 128_000, percent: 46, triggerPercent: 85 },
};

const noop = () => {};

function goal(status: GoalInfo["status"], progress = ""): GoalInfo {
  return { status, summary: TITLE, progress, elapsedMs: 248_000, requests: 37, budget: { requests: 300 }, at: Date.now() };
}

function Stack({ label, g, settings = SETTINGS }: { label: string; g: GoalInfo | null; settings?: ComposerSettings }) {
  return (
    <div className="flex w-[680px] flex-col gap-2">
      <div className="px-4 text-xs font-medium text-ink-4">{label}</div>
      <GoalBar goal={g} onPause={noop} onResume={noop} onClear={noop} onEdit={noop} />
      <ComposerStub settings={{ ...settings, goal: g }} />
    </div>
  );
}

function ComposerStub({ settings }: { settings: ComposerSettings }) {
  return (
    <Composer
      threadId="preview"
      settings={settings}
      running={false}
      stopping={false}
      onSend={async () => true}
      onStop={noop}
      onSetModel={noop}
      onSetApproval={noop}
      onSetReasoning={noop}
      onSetPlan={noop}
      loadSlash={async () => []}
      onSlash={async () => null}
      onStartGoal={async () => true}
      loadContext={async () => BREAKDOWN}
      onCompact={noop}
    />
  );
}

/** 模拟一次「进行中 → 完成」，让绿条亮起来。 */
function Completing() {
  const [g, setG] = useState<GoalInfo>(goal("active"));
  return (
    <div className="flex w-[680px] flex-col gap-2">
      <button type="button" data-complete onClick={() => setG(goal("completed", "全部跑绿"))} className="px-4 text-left text-xs font-medium text-ink-4">
        ⑥ 完成（点这里模拟）
      </button>
      <GoalBar goal={g} onPause={noop} onResume={noop} onClear={noop} onEdit={noop} />
      <ComposerStub settings={{ ...SETTINGS, goal: g }} />
    </div>
  );
}

export function GoalContextPreview() {
  const [open, setOpen] = useState(true);
  const ring = (percent: number) => ({ used: Math.round(1280 * percent), total: 128_000, percent, triggerPercent: 85 });
  return (
    <section data-preview="goal-context" className="flex flex-col gap-8 bg-pane p-8">
      <div className="grid grid-cols-2 gap-x-6 gap-y-8">
        <Stack label="① 没有目标（胶囊可点）" g={null} />
        <Stack label="③ 进行中" g={goal("active", "已补 settings/get、settings/set；正在跑 cargo test")} />
        <Stack label="⑤ 已暂停" g={goal("paused")} />
        <Completing />
      </div>
      <div className="flex items-end gap-10 px-4">
        {[14, 76, 88].map((p) => (
          <div key={p} className="flex flex-col items-center gap-1">
            <ContextRing usage={ring(p)} open={false} onOpenChange={noop} load={async () => BREAKDOWN} onCompact={noop} />
            <span className="text-[11px] text-ink-4">{p}%</span>
          </div>
        ))}
      </div>
      <div className="flex h-[700px] w-[680px] flex-col justify-end">
        <div className="mx-4 mb-3 flex justify-end">
          <ContextRing usage={SETTINGS.context} open={open} onOpenChange={setOpen} load={async () => BREAKDOWN} onCompact={noop} />
        </div>
      </div>
    </section>
  );
}

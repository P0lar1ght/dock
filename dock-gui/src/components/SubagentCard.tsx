// 对话流里的子代理卡（`task` 工具）。设计：wonder-ui/chat/SubagentCard（she–pet 八态）、
// wonder-ui/chat/SubagentGroup（pah 并行 / lof 全部结束）。
//
// 状态、用时、工具数、此刻在做什么都来自 Dock 的子代理投影（`useSubagents`），卡片按
// 派它的那次 `task` 调用（toolCallId）找到它。找不到（Dock 重启过、旧会话）就只按
// 工具结果画，并说明过程没有保留。点卡片在右侧子代理面板打开它的过程。

import { useEffect, useState } from "react";
import { Bot, ChevronRight } from "lucide-react";
import {
  currentActivity,
  elapsedMs,
  subagentForToolCall,
  toolCallCount,
  type Subagent,
  type SubagentActivity,
  type ThreadState,
  type TurnItem,
} from "dock-core";
import { store, useSubagents } from "../dock/store";
import { toolLine } from "./ToolCard";

type ToolItem = Extract<TurnItem, { kind: "tool" }>;

/** 这次调用派了子代理。 */
export function isSubagentCall(item: TurnItem): item is ToolItem {
  return item.kind === "tool" && item.name === "task" && !item.arguments.reload_roster;
}

/** 每秒跳一次的当前时间；不在跑就不跳。 */
export function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [active]);
  return now;
}

/** 子代理的用时：`1:24`（设计用分:秒）。 */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

const DOING: Record<string, string> = {
  Read: "正在读",
  List: "正在看",
  Glob: "正在找",
  Search: "正在搜",
  Bash: "正在跑",
  Edit: "正在改",
  Creating: "正在写",
  Fetch: "正在抓取",
  "Web Search": "正在搜索",
  Todo: "正在更新待办",
};

/** 「正在读 src/dock/store.ts」/「思考中」/「回复中」。`short` 只留文件名（组卡一行放不下）。 */
export function activityText(activity: SubagentActivity | null, short = false): string {
  if (!activity) return "运行中";
  if (activity.kind === "thinking") return "思考中";
  if (activity.kind === "replying") return "回复中";
  const { verb, detail, summary } = toolLine(activity.toolName, activity.arguments);
  // 有描述（bash）就说描述；`short` 只裁路径，描述不裁。
  const shown = summary ?? (short ? detail.split("/").filter(Boolean).pop() ?? detail : detail);
  return [DOING[verb] ?? `正在用 ${verb}`, shown].filter(Boolean).join(" ");
}

/** 停下的子代理在卡片和面板里的叫法。 */
export const SETTLED_LABEL: Record<string, string> = {
  idle: "已回报",
  completed: "完成",
  failed: "失败",
  cancelled: "已取消",
};

/** Dock 没有这个子代理的投影时，从 `task` 的结果里取它的回复（去掉 Dock 加的结尾）。 */
function resultText(output: string): string {
  if (/^Subagent (started in background|took longer)/.test(output)) return "";
  const cut = output.indexOf("<subagent_result>");
  return (cut >= 0 ? output.slice(0, cut) : output).trim();
}

export function RolePill({ text, small = false }: { text: string; small?: boolean }) {
  return (
    <span
      className={`flex shrink-0 items-center rounded-full bg-[#e8e8ec] font-medium leading-none text-[#3a3a3c] ${
        small ? "h-[18px] px-1.5 text-[10px]" : "h-5 px-2 text-[11px]"
      }`}
    >
      {text}
    </span>
  );
}

const Dot = () => <span className="text-[#c7c7cc]">·</span>;

/** 一张子代理卡。 */
export function SubagentCard({ item, threadId }: { item: ToolItem; threadId: string }) {
  const sub = subagentForToolCall(useSubagents(threadId), item.id);
  const running = sub ? sub.info.status === "running" : item.status === "running";
  const now = useNow(running);
  const [open, setOpen] = useState(false);
  const role = sub?.info.role || String(item.arguments.subagent_type ?? "") || "子代理";
  const description = sub?.info.description || String(item.arguments.description ?? "");
  const status = sub?.info.status ?? (item.status === "running" ? "running" : item.status === "failed" ? "failed" : "completed");
  const failed = status === "failed";
  const background = running && item.arguments.run_in_background === true;
  const summary = sub ? sub.info.output : resultText(item.output);
  const steps = sub ? recentSteps(sub.thread) : [];
  const elapsed = sub ? elapsedMs(sub, now) : item.endedAt ? item.endedAt - item.startedAt : now - item.startedAt;
  const tools = sub ? toolCallCount(sub) : null;
  const openPanel = () => sub && store.openSubagent(threadId, sub.info.agentId);

  return (
    <div className="md-surface enter-up flex shrink-0 flex-col gap-1.5 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-3 py-2.5">
      <div className="flex items-center justify-between gap-3">
        <button
          type="button"
          disabled={!sub}
          onClick={openPanel}
          title={sub ? "在右侧查看过程" : undefined}
          className="flex flex-1 items-center gap-2.5 text-left"
        >
          <span
            className={`flex size-6 shrink-0 items-center justify-center rounded-md ${
              failed ? "bg-[#fef2f2] text-[#ff3b30]" : "bg-[#e8e8ec] text-[#1c1c1e]"
            } ${running ? "breathe" : ""}`}
          >
            <Bot size={14} />
          </span>
          <span className="flex flex-1 items-center gap-2">
            <span
              className={`shrink-0 text-xs font-semibold ${
                failed ? "text-[#c0392b]" : status === "cancelled" ? "text-[#8e8e93]" : "text-[#1c1c1e]"
              }`}
            >
              {running ? "子代理" : SETTLED_LABEL[status]}
            </span>
            <RolePill text={role} />
            {background && (
              <span className="flex h-5 shrink-0 items-center rounded-full bg-[#e8e8ec] px-2 text-[11px] font-medium leading-none text-[#8e8e93]">
                后台
              </span>
            )}
            <span className="min-w-0 truncate text-[13px] text-[#3a3a3c]">{description}</span>
          </span>
        </button>
        {/* 窄的时候先截「在做什么」，左边的状态和角色胶囊不让（以前左边被挤到比胶囊还窄，胶囊叠到右边字上）。 */}
        <span className="flex min-w-0 items-center gap-1.5 whitespace-nowrap text-[11px]">
          {running && sub && (
            <>
              <span className="min-w-0 max-w-[260px] truncate text-[#636366]">{activityText(currentActivity(sub))}</span>
              <Dot />
            </>
          )}
          {status === "idle" && (
            <>
              <span className="text-[#8e8e93]">空闲</span>
              <Dot />
            </>
          )}
          {tools !== null && status !== "cancelled" && (
            <>
              <span className="text-[#8e8e93]">{tools} 次工具</span>
              <Dot />
            </>
          )}
          <span className={`font-mono ${running ? "font-medium text-accent" : "text-[#8e8e93]"}`}>{clock(elapsed)}</span>
          {running && steps.length > 0 && (
            <button
              type="button"
              aria-label={open ? "收起步骤" : "展开最近几步"}
              onClick={() => setOpen((o) => !o)}
              className="motion-colors -mr-1 ml-0.5 flex size-5 items-center justify-center rounded text-[#8e8e93] hover:bg-hover"
            >
              <ChevronRight size={13} className="rotate-open" data-open={open} />
            </button>
          )}
        </span>
      </div>
      {running && steps.length > 0 && (
        // 设计 ona：运行中展开，缩进列出最近三步 + 「查看完整过程 ›」。
        <div className="fold" data-open={open} aria-hidden={!open}>
          <div>
            <div className="flex flex-col gap-0.5 pl-[34px] pt-0.5">
              {steps.map((step) => (
                <MiniStep key={step.id} step={step} />
              ))}
              <button type="button" onClick={openPanel} className="w-fit pt-0.5 text-left text-xs font-medium text-accent hover:underline">
                查看完整过程 ›
              </button>
            </div>
          </div>
        </div>
      )}
      {failed && (
        <div className="pl-[34px] text-xs text-[#c0392b]">{sub?.info.error || "子代理出错了，Dock 没给原因。"}</div>
      )}
      {!running && !failed && status !== "cancelled" && summary && (
        <div className="line-clamp-2 whitespace-pre-wrap break-words pl-[34px] text-xs leading-normal text-[#8e8e93]">{summary}</div>
      )}
      {!sub && !running && <div className="pl-[34px] text-[11px] text-[#8e8e93]">过程没有保留（Dock 重启过）</div>}
    </div>
  );
}

/** 一步并行派出的好几个子代理合成一张（设计 pah；全部结束后默认折起，lof）。 */
export function SubagentGroup({ items, threadId }: { items: ToolItem[]; threadId: string }) {
  const state = useSubagents(threadId);
  const subs = items.map((item) => ({ item, sub: subagentForToolCall(state, item.id) }));
  const runningCount = subs.filter(({ item, sub }) => (sub ? sub.info.status === "running" : item.status === "running")).length;
  const now = useNow(runningCount > 0);
  const [open, setOpen] = useState<boolean | null>(null);
  const expanded = open ?? runningCount > 0;
  const anyFailed = subs.some(({ sub }) => sub?.info.status === "failed");

  return (
    <div className="md-surface enter-up flex shrink-0 flex-col gap-2 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-3 py-2.5">
      <button type="button" onClick={() => setOpen(!expanded)} className="flex items-center justify-between text-left">
        <span className="text-xs font-semibold text-[#1c1c1e]">
          {runningCount > 0
            ? `${items.length} 个子代理并行 · ${runningCount} 个运行中`
            : `${items.length} 个子代理 · ${anyFailed ? "有失败" : "完成"}`}
        </span>
        <ChevronRight size={13} className="rotate-open text-[#8e8e93]" data-open={expanded} />
      </button>
      <div className="fold" data-open={expanded} aria-hidden={!expanded}>
        <div>
          <div className="flex flex-col gap-1.5">
            {subs.map(({ item, sub }) => (
              <GroupRow key={item.id} item={item} sub={sub} now={now} threadId={threadId} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

function GroupRow({ item, sub, now, threadId }: { item: ToolItem; sub: Subagent | null; now: number; threadId: string }) {
  const status = sub?.info.status ?? (item.status === "running" ? "running" : "completed");
  const running = status === "running";
  const dot = running ? "bg-[#34c759]" : status === "failed" ? "bg-[#ff3b30]" : "bg-[#8e8e93]";
  const right = sub
    ? running
      ? `${activityText(currentActivity(sub), true)} · ${clock(elapsedMs(sub, now))}`
      : `${SETTLED_LABEL[status]} · ${clock(elapsedMs(sub, now))}`
    : running
      ? "运行中"
      : "完成";
  return (
    <button
      type="button"
      disabled={!sub}
      onClick={() => sub && store.openSubagent(threadId, sub.info.agentId)}
      className="motion-colors -mx-1 flex items-center justify-between gap-2 rounded-md px-1 text-left enabled:hover:bg-hover"
    >
      <span className="flex min-w-0 flex-1 items-center gap-2">
        <span className={`size-1.5 shrink-0 rounded-full ${dot}`} />
        <RolePill text={sub?.info.role || String(item.arguments.subagent_type ?? "子代理")} />
        <span className="min-w-0 truncate text-[13px] text-[#3a3a3c]">{sub?.info.description || String(item.arguments.description ?? "")}</span>
      </span>
      <span className={`shrink-0 text-[11px] ${status === "failed" ? "text-[#c0392b]" : "text-[#8e8e93]"}`}>{right}</span>
    </button>
  );
}

type StepItem = Extract<TurnItem, { kind: "tool" }>;

/** 子代理最近的三次工具调用。 */
function recentSteps(thread: ThreadState): StepItem[] {
  const tools: StepItem[] = [];
  for (const turn of thread.turns) for (const item of turn.items) if (item.kind === "tool") tools.push(item);
  return tools.slice(-3);
}

function MiniStep({ step }: { step: StepItem }) {
  const { icon: Icon, verb, detail, summary } = toolLine(step.name, step.arguments);
  return (
    <div className="flex min-w-0 items-center gap-2 py-0.5">
      <Icon size={14} className="shrink-0 text-[#8e8e93]" />
      <span className="shrink-0 text-xs font-semibold leading-none text-[#1c1c1e]">{verb}</span>
      {summary ? (
        <span className="min-w-0 truncate text-xs leading-none text-[#3a3a3c]" title={detail}>
          {summary}
        </span>
      ) : (
        <span className="min-w-0 truncate font-mono text-[11px] leading-none text-[#636366]">{detail}</span>
      )}
    </div>
  );
}

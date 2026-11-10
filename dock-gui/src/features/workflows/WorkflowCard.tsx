// 对话流里的工作流卡（`workflow` 工具）。照子代理卡的样子画（SubagentCard / SubagentGroup）：
// 头一行是状态、名字、当前阶段、在跑几个、用时；展开按脚本声明的阶段列出它派的子代理，
// 点一行在右侧子代理面板看那个子代理的过程；结束后给结果（或失败原因）。
//
// run 只在 Dock 内存里（`workflow/updated` / `workflow/list`）：找不到（Dock 重启过、旧会话、
// `validate_only`）就不画，工具卡本身还在。

import { useState } from "react";
import { ChevronRight, MessageSquareWarning, Square, Workflow } from "lucide-react";
import {
  codeReviewOf,
  currentActivity,
  workflowElapsedMs,
  workflowForToolCall,
  workflowPhases,
  type Subagent,
  type WorkflowAgentRow,
  type WorkflowRun,
} from "dock-core";
import { store, useSubagents, useWorkflows } from "../../dock/store";
import { activityText, clock, RolePill, useNow } from "../../components/SubagentCard";
import { useFeatureHost } from "../registry";

const SETTLED: Record<string, string> = {
  completed: "工作流完成",
  failed: "工作流失败",
  cancelled: "工作流已停止",
  paused: "工作流暂停",
};

const Dot = () => <span className="text-[#c7c7cc]">·</span>;

export function WorkflowCard({ threadId, toolCallId }: { threadId: string; toolCallId: string }) {
  const run = workflowForToolCall(useWorkflows(threadId), toolCallId);
  if (!run) return null;
  return <RunCard run={run} threadId={threadId} />;
}

function RunCard({ run, threadId }: { run: WorkflowRun; threadId: string }) {
  const subs = useSubagents(threadId);
  const running = run.status === "running";
  const now = useNow(running);
  const [open, setOpen] = useState<boolean | null>(null);
  const [full, setFull] = useState(false);
  const expanded = open ?? running;
  const failed = run.status === "failed" || run.status === "paused";
  const live = run.agents.filter((a) => a.status === "running").length;
  const host = useFeatureHost();
  const review = run.status === "completed" ? codeReviewOf(run) : null;
  const latestLog = run.logs[run.logs.length - 1];

  return (
    <div className="md-surface enter-fade mt-1 flex w-full shrink-0 flex-col gap-1.5 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-3 py-2.5">
      <div className="flex items-center justify-between gap-3">
        <button type="button" onClick={() => setOpen(!expanded)} className="flex flex-1 items-center gap-2.5 text-left">
          <span
            className={`flex size-6 shrink-0 items-center justify-center rounded-md ${
              failed ? "bg-[#fef2f2] text-[#ff3b30]" : "bg-[#e8e8ec] text-[#1c1c1e]"
            } ${running ? "breathe" : ""}`}
          >
            <Workflow size={14} />
          </span>
          <span className="flex flex-1 items-center gap-2">
            <span
              className={`shrink-0 text-xs font-semibold ${
                failed ? "text-[#c0392b]" : run.status === "cancelled" ? "text-[#8e8e93]" : "text-[#1c1c1e]"
              }`}
            >
              {running ? "工作流" : SETTLED[run.status]}
            </span>
            <RolePill text={run.name} />
            <span className="min-w-0 truncate text-[13px] text-[#3a3a3c]">{run.description}</span>
          </span>
        </button>
        {/* 窄的时候先截当前阶段，左边的状态和名字胶囊不让（同 SubagentCard）。 */}
        <span className="flex min-w-0 items-center gap-1.5 whitespace-nowrap text-[11px]">
          {running && run.currentPhase && (
            <>
              <span className="min-w-0 max-w-[160px] truncate text-[#636366]">{run.currentPhase}</span>
              <Dot />
            </>
          )}
          {running && live > 0 && (
            <>
              <span className="text-[#636366]">{live} 个子代理在跑</span>
              <Dot />
            </>
          )}
          {!running && run.agents.length > 0 && (
            <>
              <span className="text-[#8e8e93]">{run.agents.length} 个子代理</span>
              <Dot />
            </>
          )}
          <span className={`font-mono ${running ? "font-medium text-accent" : "text-[#8e8e93]"}`}>{clock(workflowElapsedMs(run, now))}</span>
          {running && (
            <button
              type="button"
              aria-label="停止工作流"
              title="停止工作流（连同它派的子代理）"
              onClick={() => void store.stopWorkflow(threadId, run.runId)}
              className="motion-colors ml-0.5 flex size-5 items-center justify-center rounded text-[#8e8e93] hover:bg-hover hover:text-[#ff3b30]"
            >
              <Square size={10} fill="currentColor" />
            </button>
          )}
          <button
            type="button"
            aria-label={expanded ? "收起阶段" : "展开阶段"}
            onClick={() => setOpen(!expanded)}
            className="motion-colors -mr-1 flex size-5 items-center justify-center rounded text-[#8e8e93] hover:bg-hover"
          >
            <ChevronRight size={13} className="rotate-open" data-open={expanded} />
          </button>
        </span>
      </div>
      <div className="fold" data-open={expanded} aria-hidden={!expanded}>
        <div>
          <div className="flex flex-col gap-2 pl-[34px] pt-1">
            {workflowPhases(run).map((group) => (
              <PhaseBlock
                key={group.phase ?? "_rest"}
                run={run}
                title={group.phase}
                detail={group.detail}
                agents={group.agents}
                subs={subs.byId}
                now={now}
                threadId={threadId}
              />
            ))}
            {running && latestLog && <div className="truncate text-[11px] text-[#8e8e93]">{latestLog}</div>}
          </div>
        </div>
      </div>
      {failed && <div className="whitespace-pre-wrap break-words pl-[34px] text-xs text-[#c0392b]">{run.error || "工作流出错了，Dock 没给原因。"}</div>}
      {run.status === "completed" && run.result && (
        <div className="flex flex-col gap-0.5 pl-[34px]">
          <div
            className={`whitespace-pre-wrap break-words text-xs leading-normal text-[#636366] ${full ? "thin-scroll max-h-[360px] overflow-auto" : "line-clamp-3"}`}
          >
            {run.result}
          </div>
          <span className="flex items-center gap-3">
            <button type="button" onClick={() => setFull((f) => !f)} className="w-fit text-left text-xs font-medium text-accent hover:underline">
              {full ? "收起结果" : "展开结果"}
            </button>
            {review && review.findings.length > 0 && (
              <button
                type="button"
                onClick={() => host.openPanel(`review:${run.runId}`)}
                className="flex w-fit items-center gap-1 text-left text-xs font-medium text-accent hover:underline"
              >
                <MessageSquareWarning size={12} />
                在改动上看批注
              </button>
            )}
          </span>
        </div>
      )}
    </div>
  );
}

/** 一个阶段：标题 + 它的子代理行。走过的、正在走的、还没走到的标记不同。 */
function PhaseBlock({
  run,
  title,
  detail,
  agents,
  subs,
  now,
  threadId,
}: {
  run: WorkflowRun;
  title: string | null;
  detail: string;
  agents: WorkflowAgentRow[];
  subs: Record<string, Subagent>;
  now: number;
  threadId: string;
}) {
  const index = title ? run.phases.findIndex((p) => p.title === title) : -1;
  const current = run.currentPhase ? run.phases.findIndex((p) => p.title === run.currentPhase) : -1;
  const state = title === null ? "rest" : index === current && run.status === "running" ? "current" : index < current || run.status === "completed" ? "done" : "pending";
  const mark =
    state === "current" ? "bg-accent" : state === "done" ? "bg-[#34c759]" : state === "pending" ? "border border-[#c7c7cc] bg-transparent" : "bg-[#c7c7cc]";
  return (
    <div className="flex flex-col gap-1">
      <div className="flex min-w-0 items-center gap-2">
        <span className={`size-1.5 shrink-0 rounded-full ${mark}`} />
        <span className={`shrink-0 text-xs font-semibold ${state === "pending" ? "text-[#8e8e93]" : "text-[#1c1c1e]"}`}>{title ?? "其它"}</span>
        {detail && <span className="min-w-0 truncate text-[11px] text-[#8e8e93]">{detail}</span>}
      </div>
      {agents.length > 0 && (
        <div className="flex flex-col gap-0.5 pl-3.5">
          {agents.map((row) => (
            <AgentRow key={row.agentId} row={row} sub={subs[row.agentId] ?? null} now={now} threadId={threadId} />
          ))}
        </div>
      )}
    </div>
  );
}

/** 一个子代理（同子代理组卡的一行）：点开在右侧看它的过程。 */
function AgentRow({ row, sub, now, threadId }: { row: WorkflowAgentRow; sub: Subagent | null; now: number; threadId: string }) {
  const running = row.status === "running";
  const dot = running ? "bg-[#34c759]" : row.status === "failed" ? "bg-[#ff3b30]" : "bg-[#8e8e93]";
  const elapsed = sub && running && sub.info.startedAt > 0 ? now - sub.info.startedAt : row.durationMs;
  const right = running
    ? `${sub ? activityText(currentActivity(sub), true) : "运行中"} · ${clock(elapsed)}`
    : `${row.status === "failed" ? "失败" : row.status === "cancelled" ? "已取消" : "完成"} · ${clock(elapsed)}`;
  const summary = row.status === "completed" ? row.latestReport : null;
  return (
    <button
      type="button"
      disabled={!sub}
      onClick={() => sub && store.openSubagent(threadId, row.agentId)}
      title={sub ? "在右侧查看过程" : undefined}
      className="motion-colors -mx-1 flex flex-col rounded-md px-1 py-0.5 text-left enabled:hover:bg-hover"
    >
      <span className="flex w-full items-center justify-between gap-2">
        <span className="flex min-w-0 flex-1 items-center gap-2">
          <span className={`size-1.5 shrink-0 rounded-full ${dot}`} />
          <RolePill text={row.label || "子代理"} small />
          {/* 脚本没给描述时 Dock 拿 label 当描述：一样就别重复。 */}
          {sub && sub.info.description !== row.label && (
            <span className="min-w-0 truncate text-[12px] text-[#3a3a3c]">{sub.info.description}</span>
          )}
        </span>
        <span className={`shrink-0 text-[11px] ${row.status === "failed" ? "text-[#c0392b]" : "text-[#8e8e93]"}`}>{right}</span>
      </span>
      {(summary || (row.status === "failed" && row.latestReport)) && (
        <span className={`line-clamp-1 pl-3.5 text-[11px] ${row.status === "failed" ? "text-[#c0392b]" : "text-[#8e8e93]"}`}>
          {row.status === "failed" ? row.latestReport : summary}
        </span>
      )}
    </button>
  );
}

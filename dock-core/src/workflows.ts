// 一个线程起的工作流 run（`workflow/updated` / `workflow/list`）。纯函数、不可变更新。
//
// run 只活在 Dock 的内存里：Dock 重启后 `workflow/list` 是空的，客户端只剩对话流里
// 那次 `workflow` 调用和收尾卡。run 派的子代理照常在 `subagents.ts` 里（`toolCallId`
// 为 `null`），按 `agents[].agentId` 对上。

import type { DockEvent, WorkflowRun } from './events.ts';
import { parseWorkflowRun } from './events.ts';

export interface WorkflowsState {
  byId: Record<string, WorkflowRun>;
  /** 先起的在前。 */
  order: string[];
}

export const EMPTY_WORKFLOWS: WorkflowsState = { byId: {}, order: [] };

/** `workflow/list` 的结果铺底。 */
export function seedWorkflows(state: WorkflowsState, runs: readonly Record<string, unknown>[]): WorkflowsState {
  let next = state;
  for (const raw of runs) {
    const run = parseWorkflowRun(raw);
    if (run) next = put(next, run);
  }
  return next;
}

/** 实时推送。不是工作流的事件原样返回。 */
export function reduceWorkflows(state: WorkflowsState, event: DockEvent): WorkflowsState {
  return event.method === 'workflow/updated' ? put(state, event.run) : state;
}

// ---- 查询 ----

/** 起它的那次 `workflow` 调用对应的 run。 */
export function workflowForToolCall(state: WorkflowsState, toolCallId: string): WorkflowRun | null {
  for (const id of state.order) {
    const run = state.byId[id];
    if (run.toolCallId === toolCallId) return run;
  }
  return null;
}

/** 派了 `agentId` 这个子代理的 run。 */
export function workflowOfAgent(state: WorkflowsState, agentId: string): WorkflowRun | null {
  for (const id of state.order) {
    const run = state.byId[id];
    if (run.agents.some((a) => a.agentId === agentId)) return run;
  }
  return null;
}

export function isWorkflowActive(run: WorkflowRun): boolean {
  return run.status === 'running';
}

/** 用时：在跑时从 `startedAt` 算到 `now`，停下用快照里定住的。 */
export function workflowElapsedMs(run: WorkflowRun, now: number): number {
  if (isWorkflowActive(run) && run.startedAt > 0) return Math.max(0, now - run.startedAt);
  return run.durationMs;
}

/**
 * 按阶段分组的子代理行，阶段按脚本声明的顺序；没声明阶段、或行上的阶段不在声明里的
 * 归到最后一组（`phase` 为 `null`）。空的阶段也留着（界面要画出还没走到的阶段）。
 */
export function workflowPhases(run: WorkflowRun): { phase: string | null; detail: string; agents: WorkflowRun['agents'] }[] {
  const declared = new Set(run.phases.map((p) => p.title));
  const groups = run.phases.map((p) => ({
    phase: p.title as string | null,
    detail: p.detail,
    agents: run.agents.filter((a) => a.phase === p.title),
  }));
  const rest = run.agents.filter((a) => !a.phase || !declared.has(a.phase));
  if (rest.length > 0) groups.push({ phase: null, detail: '', agents: rest });
  return groups;
}

// ---- 内部 ----

function put(state: WorkflowsState, run: WorkflowRun): WorkflowsState {
  const id = run.runId;
  // 比手上的旧：拉列表的结果晚于推送到达时会这样，丢掉。
  if (state.byId[id] && state.byId[id].revision > run.revision) return state;
  const order = state.byId[id] ? state.order : [...state.order, id];
  return { byId: { ...state.byId, [id]: run }, order };
}

import type { SessionState } from './SessionState.js';

/**
 * 这一轮有没有真在干活的痕迹：用户消息、助手正文、工具、计划任务、子代理。
 *
 * 光有交互（权限 / 提问 / 计划审批 / MCP elicitation）不算：网关在没有轮可挂时会给这类交互
 * 单开一轮（见 Gateway `elicit/requested`），那一轮只有交互项 —— agent 停着等用户，不是在跑。
 * 和 `dock-core` 的 `turnHasActivity` 同一语义。
 */
export function turnHasActivity(state: Readonly<SessionState>, turnId: string): boolean {
  if (!turnId) return false;
  const inTurn = (item: { turnId: string }) => item.turnId === turnId;
  return state.messages.some(inTurn)
    || state.toolActivities.some(inTurn)
    || state.planActivities.some(inTurn)
    || state.subagentActivities.some(inTurn);
}

/** 这一轮还有没回应的交互。 */
export function turnHasPendingInteraction(state: Readonly<SessionState>, turnId: string): boolean {
  const pending = (item: { turnId: string; status: string }) => item.turnId === turnId && item.status === 'pending';
  return state.permissionRequests.some(pending)
    || state.userInputRequests.some(pending)
    || state.planApprovals.some(pending)
    || state.elicitations.some(pending);
}

import type { SessionState } from './SessionState.js';

/** `plan/resolve` 的三种决定：批准并执行、要求修改（模型接着改计划）、退出计划模式。 */
export type PlanDecision = 'approve' | 'revise' | 'quit';

export type SessionPlanApprovalStatus = 'pending' | 'approved' | 'revised' | 'quit' | 'cancelled';

/** 计划模式下模型提交的计划（网关 `plan/requested`），等用户批准。 */
export interface SessionPlanApproval {
  id: string;
  turnId: string;
  /** 计划正文（markdown）。 */
  body: string;
  /** 计划文件路径（展示用）。 */
  path: string;
  /** 计划文件是空的。 */
  empty: boolean;
  status: SessionPlanApprovalStatus;
  seq: number;
}

const MAX_BODY_CHARS = 40_000;

export function withPlanRequested(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
): SessionState {
  const id = text(params.planId, 160);
  if (!id) return state;
  const request: SessionPlanApproval = {
    id,
    turnId,
    body: text(params.body, MAX_BODY_CHARS),
    path: text(params.path, 1024),
    empty: params.empty === true,
    status: 'pending',
    seq
  };
  return {
    ...state,
    planApprovals: [...state.planApprovals.filter((item) => item.id !== id), request]
  };
}

export function withPlanResolved(state: SessionState, params: Record<string, unknown>): SessionState {
  const id = text(params.planId, 160);
  if (!id) return state;
  const status = resolvedStatus(text(params.decision, 16));
  return {
    ...state,
    planApprovals: state.planApprovals.map((item) => item.id === id ? { ...item, status } : item)
  };
}

/** 这一轮结束时还没批的计划：不会再等了，收起来。 */
export function settlePlansForTurn(state: SessionState, turnId: string): SessionState {
  if (!state.planApprovals.some((item) => item.turnId === turnId && item.status === 'pending')) return state;
  return {
    ...state,
    planApprovals: state.planApprovals.map((item) =>
      item.turnId === turnId && item.status === 'pending' ? { ...item, status: 'cancelled' as const } : item
    )
  };
}

/** 网关只发这三种；认不出的不当成「已退出计划」，按没批收起来。 */
function resolvedStatus(decision: string): SessionPlanApprovalStatus {
  if (decision === 'approve') return 'approved';
  if (decision === 'revise') return 'revised';
  if (decision === 'quit') return 'quit';
  return 'cancelled';
}

function text(value: unknown, max: number) {
  return typeof value === 'string' ? value.slice(0, max) : '';
}

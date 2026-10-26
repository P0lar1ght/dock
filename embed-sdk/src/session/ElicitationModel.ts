import type { SessionState } from './SessionState.js';

/** `elicit/resolve` 的三种回应（MCP elicitation）：接受、拒绝、取消。 */
export type ElicitAction = 'accept' | 'decline' | 'cancel';

export type SessionElicitationStatus = 'pending' | 'resolved' | 'cancelled';

/**
 * MCP 服务向用户要的一次确认（网关 `elicit/requested`）。网关只给说明文字，不带表单 schema：
 * 这里只能接受 / 拒绝 / 取消，填不了字段。
 */
export interface SessionElicitation {
  id: string;
  turnId: string;
  server: string;
  heading: string;
  message: string;
  status: SessionElicitationStatus;
  seq: number;
}

export function withElicitationRequested(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
): SessionState {
  const id = text(params.elicitId, 160);
  if (!id) return state;
  const request: SessionElicitation = {
    id,
    turnId,
    server: text(params.server, 200),
    heading: text(params.heading, 300),
    message: text(params.message, 8_000),
    status: 'pending',
    seq
  };
  return {
    ...state,
    elicitations: [...state.elicitations.filter((item) => item.id !== id), request]
  };
}

export function withElicitationResolved(state: SessionState, params: Record<string, unknown>): SessionState {
  const id = text(params.elicitId, 160);
  if (!id) return state;
  return {
    ...state,
    elicitations: state.elicitations.map((item) => item.id === id ? { ...item, status: 'resolved' as const } : item)
  };
}

/** 这一轮结束时还没回应的：不会再等了，收起来。 */
export function settleElicitationsForTurn(state: SessionState, turnId: string): SessionState {
  if (!state.elicitations.some((item) => item.turnId === turnId && item.status === 'pending')) return state;
  return {
    ...state,
    elicitations: state.elicitations.map((item) =>
      item.turnId === turnId && item.status === 'pending' ? { ...item, status: 'cancelled' as const } : item
    )
  };
}

function text(value: unknown, max: number) {
  return typeof value === 'string' ? value.slice(0, max) : '';
}

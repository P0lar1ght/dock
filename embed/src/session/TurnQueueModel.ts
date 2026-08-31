import type { TurnQueueItem, TurnQueueResult } from '../protocol/responses.js';
import type { SessionState } from './SessionState.js';

export interface SessionTurnQueueItem extends TurnQueueItem {
  queuedSeq: number;
}

export function projectQueuedTurn(
  state: SessionState,
  params: Record<string, unknown>,
  seq: number
) {
  const payload = object(params.queue);
  const id = text(payload.id);
  if (!id) return state;
  const current = state.turnQueue.find((item) => item.id === id);
  return upsertQueueItem(state, queueItem(payload, current, seq));
}

export function projectDequeuedTurn(state: SessionState, params: Record<string, unknown>) {
  return removeQueueItem(state, text(params.queueId));
}

export function replaceTurnQueue(state: SessionState, snapshot: TurnQueueResult) {
  const previous = new Map(state.turnQueue.map((item) => [item.id, item]));
  return {
    ...state,
    turnQueue: snapshot.items
      .filter((item) => item.status === 'queued')
      .map((item) => queueItem(item, previous.get(item.id), 0))
  };
}

export function removeQueueItem(state: SessionState, id: string) {
  if (!id || !state.turnQueue.some((item) => item.id === id)) return state;
  return { ...state, turnQueue: state.turnQueue.filter((item) => item.id !== id) };
}

function upsertQueueItem(state: SessionState, item: SessionTurnQueueItem) {
  const turnQueue = state.turnQueue.some((current) => current.id === item.id)
    ? state.turnQueue.map((current) => current.id === item.id ? item : current)
    : [...state.turnQueue, item];
  return { ...state, turnQueue };
}

function queueItem(
  payload: Record<string, unknown> | TurnQueueItem,
  current: SessionTurnQueueItem | undefined,
  seq: number
): SessionTurnQueueItem {
  const createdAt = finite(payload.createdAt) || current?.createdAt || Date.now();
  return {
    id: text(payload.id),
    threadId: text(payload.threadId) || current?.threadId || '',
    message: stringValue(payload.message) || current?.message,
    kind: text(payload.kind) === 'steer' ? 'steer' : 'queue',
    status: text(payload.status) === 'running' ? 'running' : 'queued',
    turnId: text(payload.turnId) || current?.turnId,
    createdAt,
    updatedAt: finite(payload.updatedAt) || current?.updatedAt || createdAt,
    queuedSeq: current?.queuedSeq || seq
  };
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function finite(value: unknown) {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? number : 0;
}

function text(value: unknown) {
  return String(value || '').trim();
}

function stringValue(value: unknown) {
  return value === undefined || value === null ? '' : String(value);
}

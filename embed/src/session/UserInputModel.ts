import type { RuntimeQuestion } from '../protocol/interactions.js';
import type { SessionState } from './SessionState.js';

export interface SessionUserInputRequest {
  id: string;
  turnId: string;
  kind: 'user_input';
  status: 'pending' | 'resolved' | 'cancelled';
  questions: RuntimeQuestion[];
  autoResolutionMs?: number;
  seq: number;
}

export function withUserInputRequested(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  const request: SessionUserInputRequest = {
    id: text(params.interactionId, 160),
    turnId,
    kind: 'user_input',
    status: 'pending',
    questions: questions(params.questions),
    autoResolutionMs: finite(params.autoResolutionMs),
    seq
  };
  if (!request.id || !request.questions.length) return state;
  return {
    ...state,
    userInputRequests: [
      ...state.userInputRequests.filter((item) => item.id !== request.id),
      request
    ]
  };
}

export function withUserInputResolved(
  state: SessionState,
  params: Record<string, unknown>
) {
  const id = text(params.interactionId, 160);
  if (!id) return state;
  return {
    ...state,
    userInputRequests: state.userInputRequests.map((item) =>
      item.id === id ? { ...item, status: 'resolved' as const } : item
    )
  };
}

export function settleUserInputForTurn(
  state: SessionState,
  turnId: string,
  status: string
) {
  const nextStatus: SessionUserInputRequest['status'] = status === 'cancelled'
    ? 'cancelled'
    : 'resolved';
  return {
    ...state,
    userInputRequests: state.userInputRequests.map((item) =>
      item.turnId === turnId && item.status === 'pending'
        ? { ...item, status: nextStatus }
        : item
    )
  };
}

function questions(value: unknown): RuntimeQuestion[] {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 3).flatMap((questionValue, index) => {
    const question = object(questionValue);
    const options = Array.isArray(question.options)
      ? question.options.slice(0, 3).flatMap((optionValue) => {
          const option = object(optionValue);
          const label = text(option.label, 80);
          const description = text(option.description, 300);
          return label && description ? [{
            label,
            description,
            recommended: option.recommended === true
          }] : [];
        })
      : [];
    const id = text(question.id || `question_${index + 1}`, 80);
    const header = text(question.header, 80);
    const prompt = text(question.question, 600);
    return id && header && prompt && options.length >= 2
      ? [{ id, header, question: prompt, options }]
      : [];
  });
}

function object(value: unknown) {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function text(value: unknown, max: number) {
  return String(value || '').trim().slice(0, max);
}

function finite(value: unknown) {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? number : undefined;
}

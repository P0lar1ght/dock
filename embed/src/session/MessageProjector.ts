import type { StoredMessage, TranscriptEvent } from '../protocol/responses.js';
import type { SessionMessage } from './MessageModel.js';
import type { SessionState } from './SessionState.js';
import { projectMessageAttachments } from './MessageAttachmentModel.js';

export function projectUserMessage(
  state: SessionState,
  turnId: string,
  content: string,
  seq: number,
  attachments?: unknown
) {
  const id = `${turnId}:user`;
  const current = state.messages.find((message) => message.id === id);
  const message: SessionMessage = {
    id,
    turnId,
    role: 'user',
    content,
    attachments: projectMessageAttachments(attachments),
    status: 'completed',
    startedSeq: current?.startedSeq || seq,
    updatedSeq: seq
  };
  return upsertMessage(state, message);
}

export function appendAssistantDelta(
  state: SessionState,
  turnId: string,
  delta: string,
  seq: number
) {
  const current = continuableAssistant(state.messages);
  if (current) {
    return upsertMessage(state, {
      ...current,
      content: applyAssistantDelta(current.content, delta),
      status: 'streaming',
      updatedSeq: seq
    });
  }
  if (!delta) return state;
  return upsertMessage(state, {
    id: `${turnId}:assistant:${seq}`,
    turnId,
    role: 'assistant',
    content: delta,
    status: 'streaming',
    startedSeq: seq,
    updatedSeq: seq
  });
}

export function applyAssistantDelta(current: string, delta: string) {
  if (!delta) return current;
  if (!current) return delta;
  if (delta.startsWith(current)) return delta;
  return `${current}${delta}`;
}

export function closeAssistantSegment(state: SessionState, turnId: string) {
  const current = latestAssistantMessage(state.messages, turnId) ?? continuableAssistant(state.messages);
  if (!current || current.closed) return state;
  if (current.status === 'failed' || current.status === 'cancelled') return state;
  return upsertMessage(state, {
    ...current,
    status: current.status === 'streaming' ? 'completed' : current.status,
    closed: true
  });
}

export function settleAssistantSegments(
  messages: readonly SessionMessage[],
  _turnId: string,
  status: SessionMessage['status']
) {
  return messages.map((message) => (
    message.role === 'assistant' && message.status === 'streaming' && !message.closed
      ? { ...message, status }
      : message
  ));
}

export function storedMessages(messages: readonly StoredMessage[]) {
  return messages.flatMap((message, index) => storedMessage(message, index));
}

export function hasTranscriptMessage(event: TranscriptEvent) {
  return event.method === 'item/user_message' || event.method === 'item/message_delta';
}

function continuableAssistant(messages: readonly SessionMessage[]) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role === 'user') return undefined;
    if (message.role !== 'assistant') continue;
    if (message.closed || message.status === 'failed' || message.status === 'cancelled') {
      return undefined;
    }
    return message;
  }
  return undefined;
}

function latestAssistantMessage(messages: readonly SessionMessage[], turnId: string) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.turnId === turnId && message.role === 'assistant') return message;
  }
  return undefined;
}

function upsertMessage(state: SessionState, message: SessionMessage) {
  const messages = state.messages.some((item) => item.id === message.id)
    ? state.messages.map((item) => item.id === message.id ? message : item)
    : [...state.messages, message];
  return { ...state, messages };
}

function storedMessage(message: StoredMessage, index: number) {
  if (message.role !== 'user' && message.role !== 'assistant') return [];
  const stored: SessionMessage = {
    id: message.id || `stored:${index}`,
    turnId: '',
    role: message.role,
    content: message.content,
    status: 'completed',
    startedSeq: 0,
    updatedSeq: 0
  };
  return [stored];
}

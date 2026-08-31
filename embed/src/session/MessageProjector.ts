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
  const current = latestAssistantMessage(state.messages, turnId);
  if (current?.status === 'streaming') {
    return upsertMessage(state, {
      ...current,
      content: `${current.content}${delta}`,
      updatedSeq: seq
    });
  }
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

export function closeAssistantSegment(state: SessionState, turnId: string) {
  const current = latestAssistantMessage(state.messages, turnId);
  if (!current || current.status !== 'streaming') return state;
  return upsertMessage(state, { ...current, status: 'completed' });
}

export function settleAssistantSegments(
  messages: readonly SessionMessage[],
  turnId: string,
  status: SessionMessage['status']
) {
  return messages.map((message) => message.turnId === turnId && message.status === 'streaming'
    ? { ...message, status }
    : message);
}

export function storedMessages(messages: readonly StoredMessage[]) {
  return messages.flatMap((message, index) => storedMessage(message, index));
}

export function hasTranscriptMessage(event: TranscriptEvent) {
  return event.method === 'item/user_message' || event.method === 'item/message_delta';
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

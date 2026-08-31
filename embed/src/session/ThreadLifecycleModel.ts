import type { RuntimeNotification } from '../protocol/notifications.js';
import type { SessionState } from './SessionState.js';

export function reduceThreadLifecycle(
  state: SessionState,
  notification: RuntimeNotification
): SessionState {
  if (notification.method === 'thread/deleted') {
    return { ...state, connection: 'disconnected' };
  }
  const raw = notification.params.thread;
  const thread = raw && typeof raw === 'object' && !Array.isArray(raw)
    ? raw as Record<string, unknown>
    : {};
  const restored = notification.method === 'thread/restored';
  const renamed = notification.method === 'thread/renamed';
  return {
    ...state,
    thread: {
      ...state.thread,
      title: text(thread.title) || state.thread.title,
      updatedAt: Number(thread.updatedAt) || state.thread.updatedAt,
      archivedAt: renamed
        ? state.thread.archivedAt
        : restored
        ? undefined
        : Number(thread.archivedAt) || state.thread.archivedAt || state.thread.updatedAt
    }
  };
}

function text(value: unknown) {
  return String(value || '').trim();
}

export interface RuntimeNotification {
  method: string;
  params: Record<string, unknown>;
}

export function notificationSequence(notification: RuntimeNotification) {
  return Math.max(0, Number(notification.params.seq ?? notification.params.transcriptSeq) || 0);
}

export function notificationThreadId(notification: RuntimeNotification) {
  return text(notification.params.threadId);
}

export function notificationTurnId(notification: RuntimeNotification) {
  return text(notification.params.turnId);
}

function text(value: unknown) {
  return String(value || '').trim();
}

import type { HostContextEventDetail } from './types.js';

export const HOST_CONTEXT_EVENT = 'dock:context' as const;

export function hostContextEventDetail(event: Event): HostContextEventDetail | undefined {
  if (!(event instanceof CustomEvent)) return undefined;
  const detail = event.detail;
  if (!detail || typeof detail !== 'object') return undefined;
  return detail as HostContextEventDetail;
}

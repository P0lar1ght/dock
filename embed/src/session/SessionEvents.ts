import type { SessionState } from './SessionState.js';

export type SessionListener = (state: Readonly<SessionState>) => void;

export interface SessionEvent {
  method: string;
  params: Record<string, unknown>;
  seq: number;
}

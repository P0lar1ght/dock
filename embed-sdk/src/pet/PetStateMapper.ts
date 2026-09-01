import type { ClientConnectionState } from '../client/ClientEvents.js';
import type { SessionState } from '../session/SessionState.js';
import type { PetVisualState } from './types.js';

export interface PetStateInput {
  connection: ClientConnectionState;
  session?: Readonly<SessionState>;
}

export interface PetStateSnapshot {
  visual: PetVisualState;
  label: string;
  live: boolean;
}

export function mapPetState(input: PetStateInput): PetStateSnapshot {
  switch (input.connection) {
    case 'idle':
    case 'disconnected':
      return state('sleeping', 'Dock disconnected', false);
    case 'connecting':
    case 'reconnecting':
      return state('connecting', input.connection === 'reconnecting' ? 'Dock reconnecting' : 'Dock connecting', false);
    case 'error':
      return state('error', 'Dock connection error', false);
    case 'connected':
      return mapSession(input.session);
  }
}

function mapSession(session: Readonly<SessionState> | undefined): PetStateSnapshot {
  if (!session) return state('idle', 'Dock ready', true);
  if (session.connection === 'disconnected') return state('sleeping', 'Dock disconnected', false);
  if (session.connection === 'recovering') return state('connecting', 'Dock restoring session', false);
  switch (session.activity) {
    case 'thinking':
      return state('thinking', 'Dock thinking', true);
    case 'streaming':
      return state('talking', 'Dock replying', true);
    case 'working':
      return state('working', 'Dock using a tool', true);
    case 'approval':
      return state('approval', 'Dock needs approval', true);
    case 'input':
      return state('approval', 'Dock needs your direction', true);
    case 'subagent':
      return state('subagent', 'SubAgent working', true);
    case 'success':
      return state('success', 'Dock completed', true);
    case 'error':
      return state('error', session.runtimeIssues.at(-1)?.title || 'Dock turn failed', true);
    case 'idle':
      return state('idle', 'Dock ready', true);
  }
}

function state(visual: PetVisualState, label: string, live: boolean): PetStateSnapshot {
  return { visual, label, live };
}

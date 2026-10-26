import type { ClientConnectionState } from '../client/ClientEvents.js';
import type { SessionState } from '../session/SessionState.js';
import type { PetVisualState } from './types.js';

export interface PetStateInput {
  connection: ClientConnectionState;
  session?: Readonly<SessionState>;
  /** 正在等 Dock 批准这个 Origin 的配对：首次连接的正常等待，不是出错。 */
  pairing?: boolean;
}

export interface PetStateSnapshot {
  visual: PetVisualState;
  label: string;
  live: boolean;
}

export function mapPetState(input: PetStateInput): PetStateSnapshot {
  if (input.pairing && input.connection !== 'connected') {
    return state('connecting', '等待在 Dock 里批准连接', false);
  }
  switch (input.connection) {
    case 'idle':
    case 'disconnected':
      return state('sleeping', '已断开', false);
    case 'connecting':
    case 'reconnecting':
      return state('connecting', input.connection === 'reconnecting' ? '正在重新连接' : '正在连接', false);
    case 'error':
      return state('error', '连接出错', false);
    case 'connected':
      return mapSession(input.session);
  }
}

function mapSession(session: Readonly<SessionState> | undefined): PetStateSnapshot {
  if (!session) return state('idle', '已连接', true);
  if (session.connection === 'disconnected') return state('sleeping', '已断开', false);
  if (session.connection === 'recovering') return state('connecting', '正在恢复会话', false);
  switch (session.activity) {
    case 'thinking':
      return state('thinking', '思考中', true);
    case 'streaming':
      return state('talking', '回复中', true);
    case 'working':
      return state('working', '正在用工具', true);
    case 'approval':
      return state('approval', '等你批准', true);
    case 'input':
      return state('approval', '等你回答', true);
    case 'subagent':
      return state('subagent', '子代理在跑', true);
    case 'success':
      return state('success', '已完成', true);
    case 'error':
      return state('error', session.runtimeIssues.at(-1)?.title || '这一轮失败了', true);
    case 'idle':
      return state('idle', '已连接', true);
  }
}

function state(visual: PetVisualState, label: string, live: boolean): PetStateSnapshot {
  return { visual, label, live };
}

import type { ClientConnectionEvent, ClientConnectionState } from '../client/ClientEvents.js';
import type { SessionConnectionState } from '../session/SessionState.js';

export type RecoveryPhase =
  | 'hidden'
  | 'connecting'
  | 'reconnecting'
  | 'recovering'
  | 'offline'
  | 'error';

export interface RecoveryView {
  phase: RecoveryPhase;
  visible: boolean;
  retryable: boolean;
  label: string;
  detail?: string;
}

const AUTHORIZATION_ERRORS = new Set([
  'pairing_required',
  'binding_revoked',
  'origin_not_allowed'
]);

const COMPATIBILITY_ERRORS = new Set([
  'protocol_version_mismatch',
  'capability_not_supported'
]);

export class RecoveryController {
  private connection: ClientConnectionEvent = { state: 'idle' };
  private sessionConnection?: SessionConnectionState;
  private retrying = false;
  private retryError = '';

  constructor(private readonly onChange: () => void) {}

  get clientState(): ClientConnectionState {
    return this.connection.state;
  }

  get view(): RecoveryView {
    if (this.retrying) return visible('connecting', '正在重新连接本机 Agent…');
    if (this.retryError) {
      return visible('error', '重新连接失败', this.retryError, true);
    }
    switch (this.connection.state) {
      case 'connecting':
        return visible('connecting', '正在连接本机 Agent…');
      case 'reconnecting':
        return visible('reconnecting', '连接中断，正在自动恢复…', '当前回复仍在 Gateway 中继续运行。');
      case 'disconnected':
      case 'idle':
        return visible('offline', '本机 Agent 已断开', '现有消息仍保留，可以重新连接。', true);
      case 'error':
        return connectionError(this.connection);
      case 'connected':
        if (this.sessionConnection === 'recovering') {
          return visible('recovering', '正在恢复当前会话…', '正在补齐断线期间的消息。');
        }
        if (this.sessionConnection === 'disconnected') {
          return visible('offline', '当前会话尚未恢复', '可以重新连接后继续。', true);
        }
        return hidden();
    }
  }

  updateConnection(connection: Readonly<ClientConnectionEvent>) {
    if (sameConnection(this.connection, connection)) return;
    this.connection = { ...connection };
    this.retryError = '';
    this.onChange();
  }

  updateSession(connection: SessionConnectionState | undefined) {
    if (this.sessionConnection === connection) return;
    this.sessionConnection = connection;
    this.onChange();
  }

  async retry(connect: () => Promise<unknown>) {
    if (this.retrying || !this.view.retryable) return false;
    this.retrying = true;
    this.retryError = '';
    this.onChange();
    try {
      await connect();
      return true;
    } catch {
      this.retryError = '请确认 Gateway 正在运行且当前宿主仍有授权，然后重试。';
      return false;
    } finally {
      this.retrying = false;
      this.onChange();
    }
  }
}

function connectionError(connection: Readonly<ClientConnectionEvent>): RecoveryView {
  if (AUTHORIZATION_ERRORS.has(connection.code || '')) {
    return visible('error', '本机授权已失效', '请在 Dock 终端确认浏览器配对后重试。', true);
  }
  if (COMPATIBILITY_ERRORS.has(connection.code || '')) {
    return visible('error', 'Gateway 版本不兼容', '请更新本机 Gateway 后重试。', true);
  }
  return visible('error', '无法恢复本机连接', '请确认 Gateway 正在运行，然后重试。', true);
}

function visible(phase: RecoveryPhase, label: string, detail?: string, retryable = false): RecoveryView {
  return { phase, visible: true, retryable, label, detail };
}

function hidden(): RecoveryView {
  return { phase: 'hidden', visible: false, retryable: false, label: '' };
}

function sameConnection(left: Readonly<ClientConnectionEvent>, right: Readonly<ClientConnectionEvent>) {
  return left.state === right.state && left.code === right.code && left.error === right.error;
}

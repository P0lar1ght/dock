import { normalizeGatewayUrl } from '../bootstrap/GatewayUrlPolicy.js';
import type { PairingRequestResult, TicketResult } from '../bootstrap/types.js';

export type PairingPhase = 'hidden' | 'requesting' | 'ready' | 'error';

export interface PairingView {
  phase: PairingPhase;
  visible: boolean;
  pairingRequestId?: string;
  hint?: string;
  expiresAt?: number;
  error?: string;
}

export interface PairingControllerOptions {
  gatewayUrl: string;
  request: () => Promise<PairingRequestResult>;
  waitForTicket?: (pairingRequestId: string) => Promise<TicketResult>;
  onTicket?: (ticket: string) => Promise<void> | void;
  now?: () => number;
}

const REQUEST_ID_PATTERN = /^[A-Za-z0-9-]{20,80}$/u;
const EXPIRY_MARGIN_MS = 1_000;

export class PairingController {
  private readonly gatewayUrl: string;
  private readonly now: () => number;
  private state: PairingView = hidden();
  private pending?: Promise<boolean>;
  private boundWait?: Promise<void>;
  private waitGeneration = 0;

  constructor(
    private readonly options: PairingControllerOptions,
    private readonly onChange: () => void
  ) {
    this.gatewayUrl = normalizeGatewayUrl(options.gatewayUrl);
    this.now = options.now || Date.now;
  }

  get view(): PairingView {
    return { ...this.state };
  }

  begin(force = false) {
    if (this.pending) return this.pending;
    if (!force && this.isCurrentRequestUsable()) return Promise.resolve(true);
    this.state = { phase: 'requesting', visible: true };
    this.onChange();
    const pending = this.requestPairing();
    this.pending = pending;
    void pending.finally(() => {
      if (this.pending === pending) this.pending = undefined;
    });
    return pending;
  }

  /** Resolves after TUI approval + `onTicket` (WebSocket connected). */
  waitUntilBound() {
    if (this.boundWait) return this.boundWait;
    return Promise.reject(new Error('配对尚未开始'));
  }

  clear() {
    this.waitGeneration += 1;
    this.boundWait = undefined;
    if (this.state.phase === 'hidden') return;
    this.state = hidden();
    this.onChange();
  }

  private isCurrentRequestUsable() {
    return this.state.phase === 'ready'
      && Boolean(this.state.expiresAt)
      && (this.state.expiresAt || 0) > this.now() + EXPIRY_MARGIN_MS;
  }

  private async requestPairing() {
    const generation = ++this.waitGeneration;
    try {
      const result = await this.options.request();
      const pairingRequestId = String(result.pairingRequestId || '').trim();
      if (!REQUEST_ID_PATTERN.test(pairingRequestId) || result.expiresAt <= this.now()) {
        throw new Error('invalid pairing response');
      }
      this.state = {
        phase: 'ready',
        visible: true,
        pairingRequestId,
        hint: `请在 Dock 终端确认来自 ${this.gatewayUrl} 的连接请求`,
        expiresAt: result.expiresAt
      };
      this.onChange();
      this.boundWait = this.waitForApproval(pairingRequestId, generation);
      return true;
    } catch {
      this.state = {
        phase: 'error',
        visible: true,
        error: '无法创建本机配对请求，请确认 Gateway 正在运行后重新生成。'
      };
      this.onChange();
      return false;
    }
  }

  private async waitForApproval(pairingRequestId: string, generation: number) {
    if (!this.options.waitForTicket) {
      throw new Error('配对等待未配置');
    }
    try {
      const ticket = await this.options.waitForTicket(pairingRequestId);
      if (generation !== this.waitGeneration || this.state.pairingRequestId !== pairingRequestId) {
        throw new Error('配对已取消');
      }
      await this.options.onTicket?.(ticket.ticket);
    } catch (error) {
      if (generation !== this.waitGeneration || this.state.pairingRequestId !== pairingRequestId) {
        throw error;
      }
      this.state = {
        phase: 'error',
        visible: true,
        error: '配对被拒绝或已过期，请重新生成后在 Dock 终端确认。'
      };
      this.onChange();
      throw error;
    }
  }
}

function hidden(): PairingView {
  return { phase: 'hidden', visible: false };
}

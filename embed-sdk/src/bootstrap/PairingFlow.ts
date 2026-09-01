import { DockClientError } from '../protocol/errors.js';
import type { HttpBootstrapClient } from './HttpBootstrapClient.js';
import type { TicketResult } from './types.js';

const POLL_INTERVAL_MS = 500;

export class PairingFlow {
  constructor(
    private readonly http: HttpBootstrapClient,
    private readonly application: string
  ) {}

  begin() {
    return this.http.createPairingRequest(this.application);
  }

  poll(pairingRequestId: string) {
    return this.http.pollPairingRequest(pairingRequestId);
  }

  complete(pairingRequestId: string) {
    return this.http.exchangePairingRequest(pairingRequestId);
  }

  async waitUntilApproved(
    pairingRequestId: string,
    options: { intervalMs?: number; now?: () => number } = {}
  ): Promise<TicketResult> {
    const intervalMs = options.intervalMs ?? POLL_INTERVAL_MS;
    const now = options.now || Date.now;
    for (;;) {
      const result = await this.poll(pairingRequestId);
      if (result.status === 'approved') {
        return this.complete(pairingRequestId);
      }
      if (result.status === 'denied') {
        throw new DockClientError('denied', 'pairing request was denied');
      }
      if (result.status === 'expired' || result.expiresAt <= now()) {
        throw new DockClientError('expired', 'pairing request expired');
      }
      await sleep(intervalMs);
    }
  }
}

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

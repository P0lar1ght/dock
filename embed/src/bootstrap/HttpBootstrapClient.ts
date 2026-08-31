import { clientError, DockClientError } from '../protocol/errors.js';
import type { HttpBootstrapClientOptions, PairingRequestResult, TicketResult } from './types.js';
import { normalizeGatewayUrl } from './GatewayUrlPolicy.js';

export class HttpBootstrapClient {
  readonly gatewayUrl: string;
  private readonly fetchImpl: typeof globalThis.fetch;

  constructor(options: HttpBootstrapClientOptions) {
    this.gatewayUrl = normalizeGatewayUrl(options.gatewayUrl);
    this.fetchImpl = options.fetch || globalThis.fetch.bind(globalThis);
  }

  createPairingRequest(application: string) {
    return this.post<PairingRequestResult>('/v1/pairing/requests', { application });
  }

  exchangePairingRequest(pairingRequestId: string, token: string) {
    return this.post<TicketResult>('/v1/pairing/exchanges', { pairingRequestId, token });
  }

  requestConnectionTicket(application: string) {
    return this.post<TicketResult>('/v1/connection/tickets', { application });
  }

  private async post<T>(path: string, body: Record<string, unknown>): Promise<T> {
    let response: Response;
    try {
      response = await this.fetchImpl(`${this.gatewayUrl}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body)
      });
    } catch (error) {
      throw new DockClientError(
        'gateway_unreachable',
        error instanceof Error ? error.message : 'Dock Gateway is unreachable'
      );
    }
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw clientError(payload, 'bootstrap_failed', response.status);
    }
    return payload as T;
  }
}

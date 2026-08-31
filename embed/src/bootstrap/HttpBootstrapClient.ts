import { clientError, DockClientError } from '../protocol/errors.js';
import type {
  HttpBootstrapClientOptions,
  PairingPollResult,
  PairingRequestResult,
  TicketResult
} from './types.js';
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

  pollPairingRequest(pairingRequestId: string) {
    return this.get<PairingPollResult>(
      `/v1/pairing/requests/${encodeURIComponent(pairingRequestId)}`
    );
  }

  exchangePairingRequest(pairingRequestId: string) {
    return this.post<TicketResult>('/v1/pairing/exchanges', { pairingRequestId });
  }

  requestConnectionTicket(application: string) {
    return this.post<TicketResult>('/v1/connection/tickets', { application });
  }

  private async get<T>(path: string): Promise<T> {
    return this.send<T>(path, { method: 'GET' });
  }

  private async post<T>(path: string, body: Record<string, unknown>): Promise<T> {
    return this.send<T>(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
  }

  private async send<T>(path: string, init: RequestInit): Promise<T> {
    let response: Response;
    try {
      response = await this.fetchImpl(`${this.gatewayUrl}${path}`, init);
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

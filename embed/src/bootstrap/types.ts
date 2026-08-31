export interface PairingRequestResult {
  pairingRequestId: string;
  expiresAt: number;
}

export interface TicketResult {
  ticket: string;
  expiresAt: number;
}

export type PairingStatus = 'pending' | 'approved' | 'denied' | 'expired';

export interface PairingPollResult {
  status: PairingStatus;
  expiresAt: number;
  ticket?: string;
}

export interface HttpBootstrapClientOptions {
  gatewayUrl: string;
  fetch?: typeof globalThis.fetch;
}

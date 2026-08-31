export interface PairingRequestResult {
  pairingRequestId: string;
  expiresAt: number;
}

export interface TicketResult {
  ticket: string;
  expiresAt: number;
}

export interface HttpBootstrapClientOptions {
  gatewayUrl: string;
  fetch?: typeof globalThis.fetch;
}

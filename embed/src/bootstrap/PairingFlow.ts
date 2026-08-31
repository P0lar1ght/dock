import type { HttpBootstrapClient } from './HttpBootstrapClient.js';

export class PairingFlow {
  constructor(
    private readonly http: HttpBootstrapClient,
    private readonly application: string
  ) {}

  begin() {
    return this.http.createPairingRequest(this.application);
  }

  complete(pairingRequestId: string, token: string) {
    return this.http.exchangePairingRequest(pairingRequestId, token);
  }
}

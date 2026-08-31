import type { HttpBootstrapClient } from './HttpBootstrapClient.js';

export class TicketFlow {
  constructor(
    private readonly http: HttpBootstrapClient,
    private readonly application: string
  ) {}

  acquire() {
    return this.http.requestConnectionTicket(this.application);
  }
}

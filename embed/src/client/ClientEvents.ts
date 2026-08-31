export type ClientConnectionState =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'disconnected'
  | 'error';

export interface ClientConnectionEvent {
  state: ClientConnectionState;
  code?: string;
  error?: string;
}

export type ClientConnectionListener = (event: Readonly<ClientConnectionEvent>) => void;

export class ClientEventHub {
  private readonly listeners = new Set<ClientConnectionListener>();
  private currentValue: ClientConnectionEvent = { state: 'idle' };

  get current() {
    return this.currentValue;
  }

  set(state: ClientConnectionState, error?: unknown) {
    const message = error instanceof Error ? error.message : error ? String(error) : undefined;
    const code = error && typeof error === 'object' && 'code' in error
      ? String((error as { code: unknown }).code)
      : undefined;
    if (this.currentValue.state === state && this.currentValue.code === code && this.currentValue.error === message) return;
    this.currentValue = { state, code, error: message };
    for (const listener of this.listeners) listener(this.currentValue);
  }

  onChange(listener: ClientConnectionListener) {
    this.listeners.add(listener);
    listener(this.currentValue);
    return () => this.listeners.delete(listener);
  }

  clear() {
    this.listeners.clear();
  }
}

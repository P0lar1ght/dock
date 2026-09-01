import { TransportError } from './TransportError.js';

export interface WebSocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: 'open', listener: () => void, options?: AddEventListenerOptions): void;
  addEventListener(type: 'message', listener: (event: MessageEvent) => void): void;
  addEventListener(type: 'error', listener: () => void, options?: AddEventListenerOptions): void;
  addEventListener(type: 'close', listener: (event: CloseEvent) => void): void;
}

export type WebSocketFactory = (url: string) => WebSocketLike;

export class WebSocketTransport {
  private messageListeners = new Set<(data: string) => void>();
  private closeListeners = new Set<(error: TransportError) => void>();

  private constructor(private readonly socket: WebSocketLike) {
    socket.addEventListener('message', (event) => {
      const data = typeof event.data === 'string' ? event.data : String(event.data);
      for (const listener of this.messageListeners) listener(data);
    });
    socket.addEventListener('close', (event) => {
      const error = new TransportError('connection_closed', event.reason || 'Gateway connection closed');
      for (const listener of this.closeListeners) listener(error);
    });
  }

  static connect(url: string, factory: WebSocketFactory = (value) => new WebSocket(value)) {
    return new Promise<WebSocketTransport>((resolve, reject) => {
      const socket = factory(url);
      socket.addEventListener('open', () => resolve(new WebSocketTransport(socket)), { once: true });
      socket.addEventListener('error', () => reject(
        new TransportError('websocket_open_failed', 'Failed to open Dock Gateway WebSocket')
      ), { once: true });
    });
  }

  send(data: string) {
    if (this.socket.readyState !== 1) {
      throw new TransportError('connection_not_open', 'Gateway WebSocket is not open');
    }
    this.socket.send(data);
  }

  onMessage(listener: (data: string) => void) {
    this.messageListeners.add(listener);
    return () => this.messageListeners.delete(listener);
  }

  onClose(listener: (error: TransportError) => void) {
    this.closeListeners.add(listener);
    return () => this.closeListeners.delete(listener);
  }

  close() {
    this.socket.close(1000, 'client disconnect');
  }
}

export function gatewayWebSocketUrl(gatewayUrl: string) {
  const url = new URL('/api/ws', gatewayUrl);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.toString();
}

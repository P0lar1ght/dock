import { DockClientError } from '../protocol/errors.js';
import type { JsonRpcRequest, JsonRpcResponse } from '../protocol/requests.js';
import type { RuntimeNotification } from '../protocol/notifications.js';
import type { WebSocketTransport } from './WebSocketTransport.js';

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

type RequestHandler = (params: Record<string, unknown>) => unknown | Promise<unknown>;

export class JsonRpcPeer {
  private sequence = 0;
  private pending = new Map<string, PendingRequest>();
  private notificationListeners = new Set<(notification: RuntimeNotification) => void>();
  private closeListeners = new Set<(error: Error) => void>();
  private requestHandlers = new Map<string, RequestHandler>();
  private removeMessageListener: () => void;
  private removeCloseListener: () => void;

  constructor(
    private readonly transport: WebSocketTransport,
    private readonly timeoutMs = 10_000
  ) {
    this.removeMessageListener = transport.onMessage((data) => this.receive(data));
    this.removeCloseListener = transport.onClose((error) => this.rejectAll(error));
  }

  request<T>(method: string, params: Record<string, unknown> = {}) {
    const id = `sdk-${Date.now()}-${++this.sequence}`;
    const request: JsonRpcRequest = { id, method, params };
    return new Promise<T>((resolve, reject) => {
      const timeoutMs = method.startsWith('imageInputs/')
        ? Math.max(this.timeoutMs, 60_000)
        : this.timeoutMs;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new DockClientError('request_timeout', `${method} timed out`));
      }, timeoutMs);
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
      try {
        this.transport.send(JSON.stringify(request));
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(error);
      }
    });
  }

  close() {
    this.removeMessageListener();
    this.removeCloseListener();
    this.rejectAll(new DockClientError('client_closed', 'Dock client closed'));
    this.transport.close();
  }

  onNotification(listener: (notification: RuntimeNotification) => void) {
    this.notificationListeners.add(listener);
    return () => this.notificationListeners.delete(listener);
  }

  onClose(listener: (error: Error) => void) {
    this.closeListeners.add(listener);
    return () => this.closeListeners.delete(listener);
  }

  handleRequest(method: string, handler: RequestHandler) {
    if (this.requestHandlers.has(method)) {
      throw new Error(`JSON-RPC request handler already registered: ${method}`);
    }
    this.requestHandlers.set(method, handler);
    return () => {
      if (this.requestHandlers.get(method) === handler) this.requestHandlers.delete(method);
    };
  }

  private receive(data: string) {
    let response: JsonRpcResponse | RuntimeNotification | JsonRpcRequest;
    try {
      response = JSON.parse(data) as JsonRpcResponse;
    } catch {
      return;
    }
    if ('method' in response && typeof response.method === 'string') {
      if ('id' in response && response.id) {
        void this.receiveRequest(response as JsonRpcRequest);
      } else {
        const notification: RuntimeNotification = {
          method: response.method,
          params: 'params' in response && response.params && typeof response.params === 'object'
            ? response.params as Record<string, unknown>
            : {}
        };
        for (const listener of this.notificationListeners) listener(notification);
      }
      return;
    }
    if (!('id' in response) || !response.id) return;
    const pending = this.pending.get(response.id);
    if (!pending) {
      return;
    }
    clearTimeout(pending.timer);
    this.pending.delete(response.id);
    if (response.error) {
      const stableCode = String(response.error.details?.code || 'gateway_rpc_error');
      pending.reject(new DockClientError(stableCode, response.error.message));
      return;
    }
    pending.resolve(response.result);
  }

  private async receiveRequest(request: JsonRpcRequest) {
    const handler = this.requestHandlers.get(request.method);
    if (!handler) {
      this.sendResponse({
        id: request.id,
        error: {
          code: -32601,
          message: 'Method is not enabled for Gateway callbacks',
          details: { code: 'host_tool_method_not_allowed' }
        }
      });
      return;
    }
    try {
      const result = await handler(
        request.params && typeof request.params === 'object' ? request.params : {}
      );
      this.sendResponse({ id: request.id, result });
    } catch (error) {
      const code = error instanceof DockClientError
        ? error.code
        : 'host_tool_handler_error';
      const message = error instanceof DockClientError
        ? error.message.slice(0, 512)
        : 'Host Tool handler failed';
      this.sendResponse({
        id: request.id,
        error: { code: -32003, message, details: { code } }
      });
    }
  }

  private sendResponse(response: JsonRpcResponse) {
    try {
      this.transport.send(JSON.stringify(response));
    } catch {
      // Transport close owns pending request rejection and reconnection.
    }
  }

  private rejectAll(error: Error) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    for (const listener of this.closeListeners) listener(error);
  }
}

import type { HostContextProvider } from '../host/types.js';
import type { WebSocketFactory } from '../transport/WebSocketTransport.js';
import type { ReconnectPolicyOptions } from '../transport/ReconnectPolicy.js';
import type { StorageLike } from './ThreadStore.js';

/** Construction options for a product-neutral browser SDK connection. */
export interface DockClientOptions {
  application: string;
  gatewayUrl?: string;
  requestTimeoutMs?: number;
  fetch?: typeof globalThis.fetch;
  webSocketFactory?: WebSocketFactory;
  storage?: StorageLike | null;
  reconnect?: false | Partial<ReconnectPolicyOptions>;
  contextProvider?: HostContextProvider;
  collectBrowserContext?: boolean;
}

export interface CreateThreadOptions {
  workspaceId?: string;
  title?: string;
}

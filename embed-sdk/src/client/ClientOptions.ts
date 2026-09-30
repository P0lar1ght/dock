import type { HostContextProvider } from '../host/types.js';
import type { WebSocketFactory } from '../transport/WebSocketTransport.js';
import type { ReconnectPolicyOptions } from '../transport/ReconnectPolicy.js';
import type { StorageLike } from './ThreadStore.js';

/** Construction options for a product-neutral browser SDK connection. */
export interface DockClientOptions {
  application: string;
  gatewayUrl?: string;
  /**
   * Device token from `dock device add` for a remote gateway (`dock serve --remote`).
   * When set, `connect()` authenticates with it instead of pairing, `gatewayUrl`
   * is required and must be `https://` (or loopback `http://`). Kept in memory only.
   */
  deviceToken?: string;
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

import type { HostContextItem } from '../protocol/context.js';

export type HostContextReason = 'turn_start' | 'manual_refresh';

export interface HostContextRequest {
  threadId: string;
  userMessage: string;
  reason: HostContextReason;
}

export type HostContextProvider = (
  request: Readonly<HostContextRequest>
) => readonly HostContextItem[] | Promise<readonly HostContextItem[]>;

export interface HostContextClearOptions {
  source: string;
}

export interface HostContextEventDetail {
  mode: 'replace' | 'remove';
  source: string;
  items?: readonly HostContextItem[];
}

export interface HostBrowserEnvironment {
  title: string;
  origin: string;
  pathname: string;
  language: string;
  timeZone: string;
  visibility: string;
}

export interface HostBridgeOptions {
  collectBrowserContext?: boolean;
  contextProvider?: HostContextProvider;
  browserEnvironment?: () => HostBrowserEnvironment | undefined;
}

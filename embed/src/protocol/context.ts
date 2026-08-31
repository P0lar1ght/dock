export type ContextJsonValue =
  | string
  | number
  | boolean
  | null
  | ContextJsonValue[]
  | { [key: string]: ContextJsonValue };

export interface HostContextItem {
  id?: string;
  type: string;
  title?: string;
  summary?: string;
  source: string;
  entityRef?: Record<string, ContextJsonValue>;
  data?: Record<string, ContextJsonValue>;
  priority?: number;
  ttlMs?: number;
  timestamp?: number;
  truncated?: boolean;
  originalSize?: number;
}

export interface NormalizedContextItem extends Required<Pick<HostContextItem,
  'id' | 'type' | 'source' | 'data' | 'timestamp'>>,
  Omit<HostContextItem, 'id' | 'type' | 'source' | 'data' | 'timestamp'> {}

export interface TurnContextEnvelope {
  contextItems: readonly NormalizedContextItem[];
  contextSources: readonly string[];
}

export interface ContextUpdateRequest {
  threadId: string;
  workspaceId: string;
  mode: 'replace' | 'remove';
  source: string;
  items?: readonly NormalizedContextItem[];
}

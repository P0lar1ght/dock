export interface InitializeResult {
  ok: true;
  serverInfo: { name: string; version: string };
  protocolVersion: string;
  capabilities: Record<string, boolean>;
  connection: {
    application: string;
    origin: string;
    defaultWorkspaceId: string;
    connectionLeaseId: string;
  };
}

export interface AuthorizedWorkspace {
  id: string;
  trustLevel?: 'workspace' | 'full_machine';
  default: boolean;
}

export interface WorkspaceListResult {
  workspaces: AuthorizedWorkspace[];
  defaultWorkspaceId: string;
}

export interface ClientConnectionSnapshot {
  initialize: InitializeResult;
  workspaces: AuthorizedWorkspace[];
  defaultWorkspaceId: string;
}

export interface McpReloadResult {
  ok: true;
  serverCount: number;
}

export interface ThreadSummary {
  id: string;
  title: string;
  workspaceId: string;
  createdAt: number;
  updatedAt: number;
  archivedAt?: number;
}

export interface ThreadListResult {
  threads: ThreadSummary[];
}

export interface StoredMessage {
  id: string;
  threadId: string;
  role: string;
  content: string;
  createdAt: number;
}

export interface TranscriptEvent {
  timestamp: string;
  threadId: string;
  turnId: string;
  seq: number;
  method: string;
  payload: Record<string, unknown>;
}

export interface ThreadHistoryResult {
  thread: ThreadSummary;
  messages: StoredMessage[];
  events: TranscriptEvent[];
}

export type ContextCompactionStatus = 'running' | 'completed' | 'failed' | 'suppressed';
export type ReasoningEffort = 'none' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max';
export type ApprovalMode = 'ask' | 'auto' | 'full_access';
export type ThreadGoalStatus = 'none' | 'active' | 'paused' | 'blocked' | 'completed';

export interface ThreadContextUsage {
  estimatedTokens: number;
  maxContextTokens: number;
  inputBudgetTokens: number;
  reservedOutputTokens: number;
  usagePercent: number;
  compactionEnabled: boolean;
  canCompact: boolean;
  compactionTriggerPercent: number;
  compactionTriggerTokens: number;
  revisionSeq: number;
  lastCompaction?: {
    status: ContextCompactionStatus;
    beforeTokens: number;
    afterTokens: number;
    usagePercent: number;
    seq: number;
    failureCategory?: string;
  };
}

export interface ThreadCompactionResult {
  threadId: string;
  ok: boolean;
  status: 'running' | 'busy' | 'completed' | 'failed' | 'suppressed' | 'skipped';
  failureCategory?: string;
  checkpoint?: {
    id: string;
    beforeTokens: number;
    afterTokens: number;
    attempts: number;
    failureCategory?: string;
  };
  environment?: ThreadEnvironmentResult;
}

export interface ThreadEnvironmentResult {
  threadId: string;
  model: {
    id: string;
    label: string;
    inputModalities: Array<'text' | 'image'>;
    canChange: boolean;
    options: Array<{
      id: string;
      label: string;
      providerLabel?: string;
      available?: boolean;
      inputModalities?: Array<'text' | 'image'>;
    }>;
    discovery?: {
      status: 'idle' | 'refreshing' | 'ready' | 'partial' | 'failed';
      canRefresh: boolean;
      configuredCount: number;
      totalCount: number;
      availableCount: number;
      refreshedAt?: number;
    };
  };
  approval: {
    mode: ApprovalMode;
    canChange: boolean;
    options: ApprovalMode[];
  };
  reasoning: {
    supported: boolean;
    effort?: ReasoningEffort;
    canChange: boolean;
    options: ReasoningEffort[];
  };
  goal: {
    status: ThreadGoalStatus;
    summary: string;
    truncated: boolean;
    revision?: number;
    progressSummary?: string;
    completedSteps?: number;
    totalSteps?: number;
    continuationCount?: number;
    maxContinuations?: number;
    blockedReason?: string;
    updatedAt?: number;
    completedAt?: number;
  };
  plan: {
    enabled: boolean;
    canChange: boolean;
  };
  memory: {
    read: boolean;
    write: boolean;
    canRead: boolean;
    canWrite: boolean;
  };
  mcp: {
    enabled: boolean;
    connectedServers: number;
    totalServers: number;
    toolCount: number;
    refreshedAt?: number;
    servers: Array<{
      id: string;
      label: string;
      status: 'connected' | 'unavailable' | 'disabled';
      toolCount: number;
    }>;
  };
  rules: {
    appliedCount: number;
    order: Array<{
      order: number;
      scope: 'global' | 'workspace';
      label: string;
    }>;
  };
  context: ThreadContextUsage;
}

export interface FullAccessConfirmationResult {
  confirmationRequired: true;
  confirmationId: string;
  confirmationUrl: string;
  expiresAt: number;
  status: 'pending';
}

export type ApprovalModeChangeResult = ThreadEnvironmentResult | FullAccessConfirmationResult;

export interface ThreadStartResult {
  thread: ThreadSummary;
}

export interface ThreadRenameResult {
  ok: true;
  thread: ThreadSummary;
}

export interface ThreadArchiveResult {
  thread: ThreadSummary;
}

export interface ThreadRestoreResult {
  thread: ThreadSummary;
}

export interface ThreadDeleteResult {
  ok: true;
  thread: ThreadSummary;
}

export interface ThreadSubscriptionResult {
  ok: true;
  threadId: string;
  replayed: number;
  latestSeq: number;
}

export interface TurnSubmission {
  threadId: string;
  turnId?: string;
  queueId?: string;
  status: 'running' | 'queued';
}

export type TurnQueueKind = 'queue' | 'steer';
export type TurnQueueStatus = 'queued' | 'running';

export interface TurnQueueItem {
  id: string;
  threadId: string;
  message?: string;
  kind: TurnQueueKind;
  status: TurnQueueStatus;
  turnId?: string;
  createdAt: number;
  updatedAt: number;
}

export interface TurnQueueResult {
  active: {
    threadId: string;
    turnId: string;
    status: 'running' | 'waiting_permission';
    queueId?: string;
  } | null;
  items: TurnQueueItem[];
}

export interface SlashCatalogCommand {
  name: string;
  display: string;
  aliases?: string[];
  description: string;
  takesArgs?: boolean;
  surface?: 'gateway' | 'terminal' | 'embed' | string;
  kind?: string;
  capture?: 'viewport' | 'region' | 'reuse' | 'screen' | 'full-page' | string | null;
}

export interface SlashListResult {
  commands: SlashCatalogCommand[];
}

export type SlashExecuteKind =
  | 'submitted'
  | 'filled'
  | 'notice'
  | 'menu'
  | 'capture'
  | 'passthrough'
  | 'applied';

export interface SlashExecuteResult {
  ok: true;
  kind: SlashExecuteKind;
  fill?: string;
  menu?: string;
  notice?: { title: string; body: string };
  turn?: TurnSubmission;
}

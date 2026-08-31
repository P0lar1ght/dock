export { DOCK_EMBED_VERSION } from './version.js';
export type * from './host-tools/types.js';
export type * from './image-inputs/types.js';
export {
  DockClient,
  type CreateThreadOptions,
  type DockClientOptions
} from './client/DockClient.js';
export { AgentSession, type AgentSessionOperations } from './session/AgentSession.js';
export { McpClient } from './client/McpClient.js';
export type {
  ClientConnectionEvent,
  ClientConnectionListener,
  ClientConnectionState
} from './client/ClientEvents.js';
export type {
  ThreadLifecycleEvent,
  ThreadLifecycleListener,
  ThreadLifecycleMethod
} from './client/SessionCoordinator.js';
export { DockClientError } from './protocol/errors.js';
export type {
  ContextJsonValue,
  HostContextItem,
  NormalizedContextItem,
  TurnContextEnvelope
} from './protocol/context.js';
export type {
  HostContextClearOptions,
  HostContextProvider,
  HostContextRequest
} from './host/types.js';
export type {
  AuthorizedWorkspace,
  ClientConnectionSnapshot,
  InitializeResult,
  McpReloadResult,
  ThreadHistoryResult,
  ThreadSummary,
  TurnQueueItem,
  TurnQueueKind,
  TurnQueueResult,
  TurnQueueStatus,
  TurnSubmission,
  WorkspaceListResult
} from './protocol/responses.js';
export type { SessionMessage } from './session/MessageModel.js';
export type {
  RuntimeIssueAction,
  RuntimeIssueKind,
  SessionRuntimeIssue
} from './session/RuntimeIssueModel.js';
export type { SessionTurnQueueItem } from './session/TurnQueueModel.js';
export type { SessionToolActivity, SessionToolStatus } from './session/ToolActivityModel.js';
export type {
  SessionPermissionRequest,
  SessionPermissionRisk,
  SessionPermissionStatus
} from './session/PermissionModel.js';
export type {
  PermissionDecision,
  PermissionResolutionResult
} from './protocol/permissions.js';
export type {
  RuntimeInteractionResolutionResult,
  RuntimeQuestion,
  RuntimeQuestionOption,
  RuntimeUserInputAnswer
} from './protocol/interactions.js';
export type {
  SessionActivityState,
  SessionState,
  SessionTurn,
  SessionTurnStatus
} from './session/SessionState.js';

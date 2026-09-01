export { DOCK_EMBED_VERSION } from './version.js';
export type * from './host-tools/types.js';
export type * from './image-inputs/types.js';
export {
  DockClient,
  type CreateThreadOptions,
  type DockClientOptions
} from './client/DockClient.js';
export { AgentSession } from './session/AgentSession.js';
export { McpClient } from './client/McpClient.js';
export type { McpReloadResult } from './protocol/responses.js';
export type {
  RuntimeIssueAction,
  RuntimeIssueKind,
  SessionRuntimeIssue
} from './session/RuntimeIssueModel.js';
export type { SessionToolActivity, SessionToolStatus } from './session/ToolActivityModel.js';
export type {
  SessionPermissionRequest,
  SessionPermissionRisk,
  SessionPermissionStatus
} from './session/PermissionModel.js';
export type { PermissionDecision, PermissionResolutionResult } from './protocol/permissions.js';
export type {
  RuntimeInteractionResolutionResult,
  RuntimeQuestion,
  RuntimeQuestionOption,
  RuntimeUserInputAnswer
} from './protocol/interactions.js';
export type {
  TurnQueueItem,
  TurnQueueKind,
  TurnQueueResult,
  TurnQueueStatus,
  TurnSubmission
} from './protocol/responses.js';
export type { SessionTurnQueueItem } from './session/TurnQueueModel.js';
export { mountDock, type MountDockOptions } from './element/mount.js';
export { defineDockAgent, DOCK_AGENT_TAG } from './element/defineElement.js';
export { DockAgentElement } from './element/DockAgentElement.js';
export type { DockAgentPublicApi } from './element/publicApi.js';
export * from './pet/index.js';
export { DockClientError } from './protocol/errors.js';
export type {
  ThreadLifecycleEvent,
  ThreadLifecycleListener,
  ThreadLifecycleMethod
} from './client/SessionCoordinator.js';
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

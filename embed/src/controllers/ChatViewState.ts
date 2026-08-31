import type { SessionEnvironment } from '../session/ContextUsageModel.js';
import type { SessionMessage } from '../session/MessageModel.js';
import type { SessionPermissionRequest } from '../session/PermissionModel.js';
import type { SessionPlanActivity } from '../session/PlanActivityModel.js';
import type { SessionRuntimeIssue } from '../session/RuntimeIssueModel.js';
import type { SessionActivityState } from '../session/SessionState.js';
import type { SessionSubagentActivity } from '../session/SubagentActivityModel.js';
import type { SessionToolActivity } from '../session/ToolActivityModel.js';
import type { SessionTurnQueueItem } from '../session/TurnQueueModel.js';
import type { PermissionInteractionState } from './PermissionController.js';
import type { RuntimeIssueInteraction } from './RuntimeIssueController.js';
import type { SlashCommandSuggestion } from './SlashCommandModel.js';
import type { TurnControlView } from './TurnController.js';
import type { PendingImageInput } from '../image-inputs/PendingImageInputStore.js';
import type { SessionUserInputRequest } from '../session/UserInputModel.js';
import type { SessionGoalActivity } from '../session/GoalActivityModel.js';
import type { UserInputInteractionState } from './UserInputController.js';

export type ComposerMenu =
  'context' | 'model' | 'reasoning' | 'approval' | 'goal' | 'plan' | 'memory';

export type ActiveTurnSendMode = 'queue' | 'steer';
export type TurnIntentMode = 'default' | 'plan';

/** Immutable projection consumed by the Lit chat panel. */
export interface ChatViewState {
  messages: readonly SessionMessage[];
  toolActivities: readonly SessionToolActivity[];
  permissionRequests: readonly SessionPermissionRequest[];
  userInputRequests: readonly SessionUserInputRequest[];
  planActivities: readonly SessionPlanActivity[];
  goalActivities: readonly SessionGoalActivity[];
  subagentActivities: readonly SessionSubagentActivity[];
  runtimeIssues: readonly SessionRuntimeIssue[];
  runtimeIssueInteractions: Readonly<Record<string, RuntimeIssueInteraction>>;
  turnQueue: readonly SessionTurnQueueItem[];
  permissionInteractions: Readonly<Record<string, PermissionInteractionState>>;
  userInputInteractions: Readonly<Record<string, UserInputInteractionState>>;
  draft: string;
  pendingImages: readonly PendingImageInput[];
  sendMode: ActiveTurnSendMode;
  turnIntent: TurnIntentMode;
  composerMenu?: ComposerMenu;
  modelChanging: boolean;
  executionChanging: boolean;
  approvalConfirmationPending: boolean;
  goalDraft: string;
  goalChanging: boolean;
  environment?: SessionEnvironment;
  activeTurn: boolean;
  activity: SessionActivityState;
  sessionReady: boolean;
  busy: boolean;
  submitting: boolean;
  capturingScreenshot: boolean;
  canSend: boolean;
  slashCommands: readonly SlashCommandSuggestion[];
  slashCommandIndex: number;
  turnControl: TurnControlView;
  error?: string;
}

import type { RuntimeNotification } from '../protocol/notifications.js';
import type { PermissionDecision, PermissionResolutionResult } from '../protocol/permissions.js';
import type {
  ThreadHistoryResult,
  ThreadCompactionResult,
  ThreadEnvironmentResult,
  ThreadSummary,
  ApprovalModeChangeResult,
  ApprovalMode,
  ReasoningEffort,
  TurnQueueResult,
  TurnSubmission,
  SlashExecuteResult,
  SlashListResult
} from '../protocol/responses.js';
import type { SessionListener } from './SessionEvents.js';
import { reduceSession, restoreSession } from './SessionReducer.js';
import { initialSessionState, type SessionConnectionState, type SessionState } from './SessionState.js';
import {
  removeQueueItem,
  replaceTurnQueue
} from './TurnQueueModel.js';
import { mergeEnvironment } from './ContextUsageModel.js';
import { parseScreenshotCommand } from '../image-inputs/ScreenshotCommand.js';
import type {
  ImageTurnInput,
  StartTurnInput,
  TransientImagePreview
} from '../image-inputs/types.js';
import { TransientScreenshotPreviewStore } from './TransientScreenshotPreviewStore.js';
import type {
  RuntimeInteractionResolutionResult,
  RuntimeUserInputAnswer
} from '../protocol/interactions.js';

export interface AgentSessionOperations {
  startTurn(
    threadId: string,
    workspaceId: string,
    message: string,
    imageInputs?: readonly ImageTurnInput[],
    intent?: StartTurnInput['intent']
  ): Promise<TurnSubmission>;
  enqueueTurn(
    threadId: string,
    workspaceId: string,
    message: string,
    imageInputs?: readonly ImageTurnInput[]
  ): Promise<TurnSubmission>;
  steerTurn(
    threadId: string,
    workspaceId: string,
    message: string,
    imageInputs?: readonly ImageTurnInput[]
  ): Promise<TurnSubmission>;
  listTurnQueue(threadId: string, workspaceId: string): Promise<TurnQueueResult>;
  removeQueuedTurn(
    threadId: string,
    workspaceId: string,
    queueId: string
  ): Promise<{ threadId: string; queueId: string; removed: boolean }>;
  cancelTurn(threadId: string, workspaceId: string, turnId: string): Promise<{ cancelled: boolean }>;
  refreshEnvironment(threadId: string, workspaceId: string): Promise<ThreadEnvironmentResult>;
  compactContext(threadId: string, workspaceId: string): Promise<ThreadCompactionResult>;
  setModel(threadId: string, workspaceId: string, modelId: string): Promise<ThreadEnvironmentResult>;
  refreshModels(threadId: string, workspaceId: string): Promise<ThreadEnvironmentResult>;
  setReasoning(threadId: string, workspaceId: string, effort: ReasoningEffort): Promise<ThreadEnvironmentResult>;
  setApproval(threadId: string, workspaceId: string, mode: ApprovalMode, confirmationId?: string): Promise<ApprovalModeChangeResult>;
  setGoal(threadId: string, workspaceId: string, content: string): Promise<ThreadEnvironmentResult>;
  editGoal(
    threadId: string,
    workspaceId: string,
    content: string,
    expectedRevision: number,
    operationId: string
  ): Promise<{ goal: ThreadEnvironmentResult['goal'] }>;
  pauseGoal(
    threadId: string,
    workspaceId: string,
    expectedRevision: number,
    operationId: string
  ): Promise<{ goal: ThreadEnvironmentResult['goal'] }>;
  completeGoal(threadId: string, workspaceId: string): Promise<ThreadEnvironmentResult>;
  clearGoal(
    threadId: string,
    workspaceId: string,
    operationId: string
  ): Promise<{ goal: ThreadEnvironmentResult['goal'] }>;
  setPlanMode(threadId: string, workspaceId: string, enabled: boolean): Promise<ThreadEnvironmentResult>;
  setMemory(
    threadId: string,
    workspaceId: string,
    selection: { read?: boolean; write?: boolean }
  ): Promise<ThreadEnvironmentResult>;
  listSlashCommands(): Promise<SlashListResult>;
  executeSlash(text: string, threadId: string): Promise<SlashExecuteResult>;
  resolvePermission(
    threadId: string,
    workspaceId: string,
    requestId: string,
    turnId: string,
    decision: PermissionDecision
  ): Promise<PermissionResolutionResult>;
  respondToInteraction(
    threadId: string,
    workspaceId: string,
    interactionId: string,
    turnId: string,
    answers: RuntimeUserInputAnswer[]
  ): Promise<RuntimeInteractionResolutionResult>;
}

export class AgentSession {
  private stateValue: SessionState;
  private readonly listeners = new Set<SessionListener>();
  private readonly screenshotPreviews = new TransientScreenshotPreviewStore();

  constructor(
    thread: ThreadSummary,
    private readonly operations: AgentSessionOperations
  ) {
    this.stateValue = initialSessionState(thread);
  }

  get id() {
    return this.stateValue.thread.id;
  }

  get workspaceId() {
    return this.stateValue.thread.workspaceId;
  }

  get state(): Readonly<SessionState> {
    return this.stateValue;
  }

  get lastSeq() {
    return this.stateValue.lastSeq;
  }

  restore(history: ThreadHistoryResult) {
    this.stateValue = this.screenshotPreviews.project({
      ...restoreSession(history),
      connection: this.stateValue.connection,
      environment: this.stateValue.environment
    });
    this.emit();
  }

  setEnvironment(environment: ThreadEnvironmentResult) {
    const next = mergeEnvironment(this.stateValue.environment, environment);
    if (next === this.stateValue.environment) return;
    this.stateValue = { ...this.stateValue, environment: next };
    this.emit();
  }

  receive(notification: RuntimeNotification) {
    if (text(notification.params.threadId) !== this.id) return;
    const next = this.screenshotPreviews.project(reduceSession(this.stateValue, notification));
    if (next !== this.stateValue) {
      this.stateValue = next;
      this.emit();
    }
  }

  setConnection(connection: SessionConnectionState) {
    if (this.stateValue.connection === connection) return;
    this.stateValue = { ...this.stateValue, connection };
    this.emit();
  }

  startTurn(messageValue: string | StartTurnInput) {
    const parsed = typeof messageValue === 'string'
      ? parseScreenshotCommand(messageValue, this.latestImageTurnId())
      : messageValue;
    const message = text(typeof parsed === 'string' ? parsed : parsed.message);
    if (!message) throw new Error('message is required');
    const imageInputs = typeof parsed === 'string' ? undefined : parsed.imageInputs;
    const intent = typeof parsed === 'string' ? undefined : parsed.intent;
    return intent
      ? this.operations.startTurn(this.id, this.workspaceId, message, imageInputs, intent)
      : this.operations.startTurn(this.id, this.workspaceId, message, imageInputs);
  }

  setImagePreviews(turnId: string, previews: readonly TransientImagePreview[]) {
    this.screenshotPreviews.add(turnId, previews);
    const next = this.screenshotPreviews.project(this.stateValue);
    if (next !== this.stateValue) {
      this.stateValue = next;
      this.emit();
    }
  }

  releaseScreenshotPreviews() {
    this.screenshotPreviews.clear();
    this.stateValue = this.screenshotPreviews.project(this.stateValue);
    this.emit();
  }

  retryTurn(turnIdValue: string) {
    const turnId = requiredText(turnIdValue, 'turnId');
    const original = [...this.stateValue.messages].reverse().find((item) =>
      item.turnId === turnId && item.role === 'user'
    );
    if (!original?.content) throw new Error('Original user message was not found');
    const hadImages = Boolean(original.attachments?.length);
    return this.startTurn(hadImages ? {
      message: original.content,
      imageInputs: [{ type: 'reuse', detail: 'auto', reuseTurnId: turnId }]
    } : original.content);
  }

  messageForTurn(turnIdValue: string) {
    const turnId = text(turnIdValue);
    return [...this.stateValue.messages].reverse().find((item) =>
      item.turnId === turnId && item.role === 'user'
    )?.content;
  }

  async enqueueTurn(messageValue: string | StartTurnInput) {
    const parsed = typeof messageValue === 'string'
      ? parseScreenshotCommand(messageValue, this.latestImageTurnId())
      : messageValue;
    const message = requiredText(typeof parsed === 'string' ? parsed : parsed.message, 'message');
    const imageInputs = typeof parsed === 'string' ? undefined : parsed.imageInputs;
    const submission = await this.operations.enqueueTurn(
      this.id,
      this.workspaceId,
      message,
      imageInputs
    );
    await this.refreshTurnQueue().catch(() => undefined);
    return submission;
  }

  async steerTurn(messageValue: string | StartTurnInput) {
    const parsed = typeof messageValue === 'string'
      ? parseScreenshotCommand(messageValue, this.latestImageTurnId())
      : messageValue;
    const message = requiredText(typeof parsed === 'string' ? parsed : parsed.message, 'message');
    const imageInputs = typeof parsed === 'string' ? undefined : parsed.imageInputs;
    const submission = await this.operations.steerTurn(
      this.id,
      this.workspaceId,
      message,
      imageInputs
    );
    await this.refreshTurnQueue().catch(() => undefined);
    return submission;
  }

  private latestImageTurnId() {
    return [...this.stateValue.messages].reverse().find((item) =>
      item.role === 'user' && Boolean(item.attachments?.length)
    )?.turnId;
  }

  async refreshTurnQueue() {
    const snapshot = await this.operations.listTurnQueue(this.id, this.workspaceId);
    this.stateValue = replaceTurnQueue(this.stateValue, snapshot);
    this.emit();
    return snapshot;
  }

  async removeQueuedTurn(queueIdValue: string) {
    const queueId = requiredText(queueIdValue, 'queueId');
    const result = await this.operations.removeQueuedTurn(this.id, this.workspaceId, queueId);
    if (result.removed) {
      this.stateValue = removeQueueItem(this.stateValue, queueId);
      this.emit();
    }
    return result;
  }

  cancelTurn(turnId: string) {
    return this.operations.cancelTurn(this.id, this.workspaceId, text(turnId));
  }

  async refreshEnvironment() {
    const environment = await this.operations.refreshEnvironment(this.id, this.workspaceId);
    this.setEnvironment(environment);
    return environment;
  }

  async compactContext() {
    const result = await this.operations.compactContext(this.id, this.workspaceId);
    if (result.environment) this.setEnvironment(result.environment);
    return result;
  }

  async setModel(modelIdValue: string) {
    const modelId = requiredText(modelIdValue, 'modelId');
    const environment = await this.operations.setModel(this.id, this.workspaceId, modelId);
    this.setEnvironment(environment);
    return environment;
  }

  async refreshModels() {
    const environment = await this.operations.refreshModels(this.id, this.workspaceId);
    this.setEnvironment(environment);
    return environment;
  }

  async setReasoning(effort: ReasoningEffort) {
    const environment = await this.operations.setReasoning(this.id, this.workspaceId, effort);
    this.setEnvironment(environment);
    return environment;
  }

  async setApproval(mode: ApprovalMode, confirmationId?: string) {
    const result = await this.operations.setApproval(this.id, this.workspaceId, mode, confirmationId);
    if (!('confirmationRequired' in result)) this.setEnvironment(result);
    return result;
  }

  async setGoal(contentValue: string) {
    const content = requiredText(contentValue, 'content');
    const environment = await this.operations.setGoal(this.id, this.workspaceId, content);
    this.setEnvironment(environment);
    return environment;
  }

  startGoal(contentValue: string) {
    const content = requiredText(contentValue, 'content');
    return this.startTurn({
      message: content,
      intent: {
        mode: 'default',
        goal: {
          operation: 'start',
          objective: content,
          operationId: goalOperationId()
        }
      }
    });
  }

  async editGoal(contentValue: string, expectedRevision: number) {
    const content = requiredText(contentValue, 'content');
    const result = await this.operations.editGoal(
      this.id,
      this.workspaceId,
      content,
      expectedRevision,
      goalOperationId()
    );
    this.setGoalSnapshot(result.goal);
    return result;
  }

  async pauseGoal(expectedRevision: number) {
    const result = await this.operations.pauseGoal(
      this.id,
      this.workspaceId,
      expectedRevision,
      goalOperationId()
    );
    this.setGoalSnapshot(result.goal);
    return result;
  }

  resumeGoal(expectedRevision: number) {
    return this.startTurn({
      message: 'Resume the active Thread Goal.',
      intent: {
        mode: 'default',
        goal: {
          operation: 'resume',
          expectedRevision,
          operationId: goalOperationId()
        }
      }
    });
  }

  async completeGoal() {
    const environment = await this.operations.completeGoal(this.id, this.workspaceId);
    this.setEnvironment(environment);
    return environment;
  }

  async clearGoal() {
    const result = await this.operations.clearGoal(
      this.id,
      this.workspaceId,
      goalOperationId()
    );
    this.setGoalSnapshot(result.goal);
    return result;
  }

  async setPlanMode(enabled: boolean) {
    const environment = await this.operations.setPlanMode(this.id, this.workspaceId, enabled);
    this.setEnvironment(environment);
    return environment;
  }

  async setMemory(selection: { read?: boolean; write?: boolean }) {
    if (typeof selection.read !== 'boolean' && typeof selection.write !== 'boolean') {
      throw new Error('Memory read or write selection is required');
    }
    const environment = await this.operations.setMemory(this.id, this.workspaceId, selection);
    this.setEnvironment(environment);
    return environment;
  }

  listSlashCommands() {
    return this.operations.listSlashCommands();
  }

  executeSlash(text: string) {
    return this.operations.executeSlash(text, this.id);
  }

  resolvePermission(requestIdValue: string, decision: PermissionDecision) {
    const requestId = text(requestIdValue);
    const request = this.stateValue.permissionRequests.find((item) => item.id === requestId);
    if (!request || request.status !== 'pending') throw new Error('Pending permission request was not found');
    return this.operations.resolvePermission(this.id, this.workspaceId, requestId, request.turnId, decision);
  }

  respondToInteraction(interactionIdValue: string, answers: RuntimeUserInputAnswer[]) {
    const interactionId = requiredText(interactionIdValue, 'interactionId');
    const request = this.stateValue.userInputRequests.find((item) =>
      item.id === interactionId && item.status === 'pending'
    );
    if (!request) throw new Error('Pending user-input request was not found');
    return this.operations.respondToInteraction(
      this.id,
      this.workspaceId,
      interactionId,
      request.turnId,
      answers
    );
  }

  onChange(listener: SessionListener) {
    this.listeners.add(listener);
    listener(this.stateValue);
    return () => this.listeners.delete(listener);
  }

  private emit() {
    for (const listener of this.listeners) listener(this.stateValue);
  }

  private setGoalSnapshot(goal: ThreadEnvironmentResult['goal']) {
    if (!this.stateValue.environment) return;
    this.stateValue = {
      ...this.stateValue,
      environment: { ...this.stateValue.environment, goal }
    };
    this.emit();
  }

  private hasActiveOrQueuedTurn() {
    return this.stateValue.turnQueue.length > 0 || Object.values(this.stateValue.turns).some((turn) =>
      turn.status === 'running'
      || turn.status === 'waiting_permission'
      || turn.status === 'waiting_input'
      || turn.status === 'queued'
    );
  }
}

function text(value: unknown) {
  return String(value || '').trim();
}

function requiredText(value: unknown, field: string) {
  const result = text(value);
  if (!result) throw new Error(`${field} is required`);
  return result;
}

function goalOperationId() {
  const random = globalThis.crypto?.randomUUID?.();
  return random || `goal-${Date.now()}-${Math.random().toString(36).slice(2, 12)}`;
}

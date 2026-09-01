import {
  THREAD_ARCHIVE,
  THREAD_DELETE,
  THREAD_CONTEXT_COMPACT,
  THREAD_ENVIRONMENT_GET,
  THREAD_HISTORY,
  THREAD_LIST,
  THREAD_MODEL_SET,
  THREAD_MODEL_REFRESH,
  THREAD_MEMORY_SET,
  THREAD_REASONING_SET,
  THREAD_APPROVAL_SET,
  THREAD_GOAL_CLEAR,
  THREAD_GOAL_COMPLETE,
  THREAD_GOAL_EDIT,
  THREAD_GOAL_PAUSE,
  THREAD_GOAL_SET,
  THREAD_PLAN_SET,
  THREAD_RESTORE,
  THREAD_RENAME,
  THREAD_START,
  THREAD_SUBSCRIBE,
  THREAD_UNSUBSCRIBE
} from '../protocol/methods.js';
import type {
  ThreadArchiveResult,
  ThreadDeleteResult,
  ThreadCompactionResult,
  ThreadEnvironmentResult,
  ApprovalModeChangeResult,
  ApprovalMode,
  ReasoningEffort,
  ThreadHistoryResult,
  ThreadListResult,
  ThreadRenameResult,
  ThreadRestoreResult,
  ThreadStartResult,
  ThreadSubscriptionResult
} from '../protocol/responses.js';

export type RpcRequest = <T>(method: string, params?: Record<string, unknown>) => Promise<T>;

export class ThreadClient {
  constructor(private readonly request: RpcRequest) {}

  async list(workspaceId: string, options: { includeArchived?: boolean } = {}) {
    const result = await this.request<ThreadListResult>(THREAD_LIST, {
      workspaceId,
      ...(options.includeArchived ? { includeArchived: true } : {})
    });
    return result.threads;
  }

  async create(workspaceId: string, title?: string) {
    const result = await this.request<ThreadStartResult>(THREAD_START, {
      workspaceId,
      ...(title ? { title } : {})
    });
    return result.thread;
  }

  async rename(threadId: string, workspaceId: string, title: string) {
    const result = await this.request<ThreadRenameResult>(THREAD_RENAME, {
      threadId,
      workspaceId,
      title
    });
    return result.thread;
  }

  async archive(threadId: string, workspaceId: string) {
    const result = await this.request<ThreadArchiveResult>(THREAD_ARCHIVE, { threadId, workspaceId });
    return result.thread;
  }

  async restore(threadId: string, workspaceId: string) {
    const result = await this.request<ThreadRestoreResult>(THREAD_RESTORE, { threadId, workspaceId });
    return result.thread;
  }

  delete(threadId: string, workspaceId: string, confirmation: string) {
    return this.request<ThreadDeleteResult>(THREAD_DELETE, { threadId, workspaceId, confirmation });
  }

  history(threadId: string, workspaceId: string) {
    return this.request<ThreadHistoryResult>(THREAD_HISTORY, { threadId, workspaceId });
  }

  environment(threadId: string, workspaceId: string) {
    return this.request<ThreadEnvironmentResult>(THREAD_ENVIRONMENT_GET, { threadId, workspaceId });
  }

  compactContext(threadId: string, workspaceId: string) {
    return this.request<ThreadCompactionResult>(THREAD_CONTEXT_COMPACT, { threadId, workspaceId });
  }

  setModel(threadId: string, workspaceId: string, modelId: string) {
    return this.request<ThreadEnvironmentResult>(THREAD_MODEL_SET, {
      threadId,
      workspaceId,
      modelId
    });
  }

  refreshModels(threadId: string, workspaceId: string) {
    return this.request<ThreadEnvironmentResult>(THREAD_MODEL_REFRESH, { threadId, workspaceId });
  }

  setReasoning(threadId: string, workspaceId: string, effort: ReasoningEffort) {
    return this.request<ThreadEnvironmentResult>(THREAD_REASONING_SET, {
      threadId,
      workspaceId,
      effort
    });
  }

  setApproval(
    threadId: string,
    workspaceId: string,
    mode: ApprovalMode,
    confirmationId?: string
  ) {
    return this.request<ApprovalModeChangeResult>(THREAD_APPROVAL_SET, {
      threadId,
      workspaceId,
      mode,
      ...(confirmationId ? { confirmationId } : {})
    });
  }

  setGoal(threadId: string, workspaceId: string, content: string) {
    return this.request<ThreadEnvironmentResult>(THREAD_GOAL_SET, {
      threadId,
      workspaceId,
      content
    });
  }

  completeGoal(threadId: string, workspaceId: string) {
    return this.request<ThreadEnvironmentResult>(THREAD_GOAL_COMPLETE, { threadId, workspaceId });
  }

  editGoal(
    threadId: string,
    workspaceId: string,
    content: string,
    expectedRevision: number,
    operationId: string
  ) {
    return this.request<{ goal: ThreadEnvironmentResult['goal'] }>(THREAD_GOAL_EDIT, {
      threadId,
      workspaceId,
      content,
      expectedRevision,
      operationId
    });
  }

  pauseGoal(
    threadId: string,
    workspaceId: string,
    expectedRevision: number,
    operationId: string
  ) {
    return this.request<{ goal: ThreadEnvironmentResult['goal'] }>(THREAD_GOAL_PAUSE, {
      threadId,
      workspaceId,
      expectedRevision,
      operationId
    });
  }

  clearGoal(threadId: string, workspaceId: string, operationId: string) {
    return this.request<{ goal: ThreadEnvironmentResult['goal'] }>(THREAD_GOAL_CLEAR, {
      threadId,
      workspaceId,
      operationId
    });
  }

  setPlanMode(threadId: string, workspaceId: string, enabled: boolean) {
    return this.request<ThreadEnvironmentResult>(THREAD_PLAN_SET, {
      threadId,
      workspaceId,
      enabled
    });
  }

  setMemory(
    threadId: string,
    workspaceId: string,
    selection: { read?: boolean; write?: boolean }
  ) {
    return this.request<ThreadEnvironmentResult>(THREAD_MEMORY_SET, {
      threadId,
      workspaceId,
      ...selection
    });
  }

  subscribe(threadId: string, workspaceId: string, sinceSeq: number) {
    return this.request<ThreadSubscriptionResult>(THREAD_SUBSCRIBE, {
      threadId,
      workspaceId,
      sinceSeq
    });
  }

  unsubscribe(threadId: string, workspaceId: string) {
    return this.request<{ ok: true }>(THREAD_UNSUBSCRIBE, { threadId, workspaceId });
  }
}

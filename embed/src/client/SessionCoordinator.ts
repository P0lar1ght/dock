import type { RuntimeNotification } from '../protocol/notifications.js';
import { DockClientError } from '../protocol/errors.js';
import type { ThreadSummary } from '../protocol/responses.js';
import { AgentSession, type AgentSessionOperations } from '../session/AgentSession.js';
import type { ThreadClient } from './ThreadClient.js';
import type { ThreadStore } from './ThreadStore.js';

export type ThreadLifecycleMethod =
  | 'thread/renamed'
  | 'thread/archived'
  | 'thread/restored'
  | 'thread/deleted';

export interface ThreadLifecycleEvent {
  method: ThreadLifecycleMethod;
  thread: ThreadSummary;
  source: 'local' | 'remote';
  wasActive: boolean;
}

export type ThreadLifecycleListener = (event: Readonly<ThreadLifecycleEvent>) => void;

export interface SessionCoordinatorOptions {
  threadApi: ThreadClient;
  threadStore: ThreadStore;
  createOperations: () => AgentSessionOperations;
  requireWorkspace: (workspaceId: string) => void;
  origin: () => string;
}

export class SessionCoordinator {
  private readonly sessions = new Map<string, AgentSession>();
  private readonly subscribedThreads = new Set<string>();
  private readonly lifecycleListeners = new Set<ThreadLifecycleListener>();
  private activeWorkspaceValue = '';
  private activeSessionValue?: AgentSession;

  constructor(private readonly options: SessionCoordinatorOptions) {}

  get activeSession() {
    return this.activeSessionValue;
  }

  get activeWorkspaceId() {
    return this.activeWorkspaceValue;
  }

  list(
    workspaceId = this.requireActiveWorkspace(),
    options: { includeArchived?: boolean } = {}
  ) {
    this.options.requireWorkspace(workspaceId);
    return this.options.threadApi.list(workspaceId, options);
  }

  async create(workspaceId = this.requireActiveWorkspace(), title?: string) {
    this.options.requireWorkspace(workspaceId);
    const thread = await this.options.threadApi.create(workspaceId, title);
    return this.open(thread.id, workspaceId, thread, true);
  }

  restore(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.open(requiredText(threadId, 'threadId'), workspaceId, undefined, true);
  }

  observe(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.open(requiredText(threadId, 'threadId'), workspaceId, undefined, false);
  }

  async archive(threadIdValue: string, workspaceId = this.requireActiveWorkspace()) {
    const threadId = requiredText(threadIdValue, 'threadId');
    this.options.requireWorkspace(workspaceId);
    const thread = await this.options.threadApi.archive(threadId, workspaceId);
    this.applyLifecycle('thread/archived', thread, 'local');
    return thread;
  }

  async rename(
    threadIdValue: string,
    title: string,
    workspaceId = this.requireActiveWorkspace()
  ) {
    const threadId = requiredText(threadIdValue, 'threadId');
    this.options.requireWorkspace(workspaceId);
    const thread = await this.options.threadApi.rename(threadId, workspaceId, title);
    this.applyLifecycle('thread/renamed', thread, 'local');
    return thread;
  }

  async restoreArchived(threadIdValue: string, workspaceId = this.requireActiveWorkspace()) {
    const threadId = requiredText(threadIdValue, 'threadId');
    this.options.requireWorkspace(workspaceId);
    const thread = await this.options.threadApi.restore(threadId, workspaceId);
    this.applyLifecycle('thread/restored', thread, 'local');
    return thread;
  }

  async delete(threadIdValue: string, workspaceId = this.requireActiveWorkspace(), confirmation = '') {
    const threadId = requiredText(threadIdValue, 'threadId');
    this.options.requireWorkspace(workspaceId);
    const result = await this.options.threadApi.delete(threadId, workspaceId, confirmation);
    this.applyLifecycle('thread/deleted', result.thread, 'local');
    return result;
  }

  async switchWorkspace(workspaceId: string) {
    this.options.requireWorkspace(workspaceId);
    this.activeWorkspaceValue = workspaceId;
    this.options.threadStore.setActiveWorkspace(this.options.origin(), workspaceId);
    const threadId = this.options.threadStore.lastThread(this.options.origin(), workspaceId);
    if (!threadId) {
      this.activeSessionValue = undefined;
      return undefined;
    }
    return this.open(threadId, workspaceId, undefined, true);
  }

  get(threadId: string) {
    return this.sessions.get(text(threadId));
  }

  onLifecycleChange(listener: ThreadLifecycleListener) {
    this.lifecycleListeners.add(listener);
    return () => this.lifecycleListeners.delete(listener);
  }

  dispatch(notification: RuntimeNotification) {
    const lifecycle = lifecycleNotification(notification);
    if (lifecycle) {
      this.applyLifecycle(lifecycle.method, lifecycle.thread, 'remote');
      return;
    }
    const threadId = text(notification.params.threadId);
    if (threadId) this.sessions.get(threadId)?.receive(notification);
  }

  setDisconnected() {
    this.subscribedThreads.clear();
    for (const session of this.sessions.values()) session.setConnection('disconnected');
  }

  async recoverAfterConnect(defaultWorkspaceId: string, workspaceIds: readonly string[]) {
    this.subscribedThreads.clear();
    const allowed = new Set(workspaceIds);
    const origin = this.options.origin();
    const storedWorkspace = this.options.threadStore.activeWorkspace(origin);
    const intendedWorkspace = allowed.has(this.activeWorkspaceValue)
      ? this.activeWorkspaceValue
      : allowed.has(storedWorkspace) ? storedWorkspace : defaultWorkspaceId;
    this.activeWorkspaceValue = intendedWorkspace;

    const recoverableSessions: AgentSession[] = [];
    for (const session of this.sessions.values()) {
      if (!allowed.has(session.workspaceId)) {
        session.setConnection('disconnected');
        continue;
      }
      session.setConnection('recovering');
      recoverableSessions.push(session);
    }
    try {
      await Promise.all(recoverableSessions.map((session) => this.recoverSession(session)));
      for (const session of recoverableSessions) session.setConnection('live');
    } catch (error) {
      for (const session of recoverableSessions) session.setConnection('disconnected');
      throw error;
    }

    const lastThreadId = this.options.threadStore.lastThread(origin, intendedWorkspace);
    const existing = this.sessions.get(lastThreadId);
    if (existing && allowed.has(existing.workspaceId) && !existing.state.thread.archivedAt) {
      this.activate(existing);
      return;
    }
    if (!lastThreadId) {
      this.activeSessionValue = undefined;
      this.options.threadStore.setActiveWorkspace(origin, intendedWorkspace);
      return;
    }
    try {
      const session = await this.open(lastThreadId, intendedWorkspace, undefined, true);
      if (session.state.thread.archivedAt) {
        this.options.threadStore.clearLastThread(origin, intendedWorkspace);
        this.activeSessionValue = undefined;
      }
    } catch (error) {
      if (!isMissingThread(error)) throw error;
      this.options.threadStore.clearLastThread(origin, intendedWorkspace);
      this.activeSessionValue = undefined;
    }
  }

  private async open(
    threadId: string,
    workspaceId: string,
    knownThread: ThreadSummary | undefined,
    activate: boolean
  ) {
    this.options.requireWorkspace(workspaceId);
    const existing = this.sessions.get(threadId);
    if (existing && this.subscribedThreads.has(threadId)) {
      if (activate) this.activate(existing);
      return existing;
    }

    const [history, environment] = await Promise.all([
      this.options.threadApi.history(threadId, workspaceId),
      this.options.threadApi.environment(threadId, workspaceId)
    ]);
    const session = existing || new AgentSession(knownThread || history.thread, this.options.createOperations());
    session.setConnection('recovering');
    session.restore(history);
    session.setEnvironment(environment);
    this.sessions.set(threadId, session);
    await this.options.threadApi.subscribe(threadId, workspaceId, session.lastSeq);
    this.subscribedThreads.add(threadId);
    await session.refreshTurnQueue();
    session.setConnection('live');
    if (activate) this.activate(session);
    return session;
  }

  private activate(session: AgentSession) {
    this.activeSessionValue = session;
    this.activeWorkspaceValue = session.workspaceId;
    const origin = this.options.origin();
    this.options.threadStore.setActiveWorkspace(origin, session.workspaceId);
    this.options.threadStore.setLastThread(origin, session.workspaceId, session.id);
  }

  private applyLifecycle(
    method: ThreadLifecycleMethod,
    thread: ThreadSummary,
    source: ThreadLifecycleEvent['source']
  ) {
    const wasActive = this.activeSessionValue?.id === thread.id;
    this.sessions.get(thread.id)?.receive({
      method,
      params: { threadId: thread.id, workspaceId: thread.workspaceId, thread }
    });
    if (method === 'thread/archived' || method === 'thread/deleted') {
      this.sessions.get(thread.id)?.releaseScreenshotPreviews();
      if (wasActive) {
        this.activeSessionValue = undefined;
        this.options.threadStore.clearLastThread(this.options.origin(), thread.workspaceId);
      }
      if (method === 'thread/deleted') {
        this.sessions.delete(thread.id);
        this.subscribedThreads.delete(thread.id);
      }
    }
    const event = { method, thread, source, wasActive } satisfies ThreadLifecycleEvent;
    for (const listener of this.lifecycleListeners) listener(event);
  }

  private async recoverSession(session: AgentSession) {
    const [history, environment] = await Promise.all([
      this.options.threadApi.history(session.id, session.workspaceId),
      this.options.threadApi.environment(session.id, session.workspaceId)
    ]);
    session.restore(history);
    session.setEnvironment(environment);
    await this.options.threadApi.subscribe(session.id, session.workspaceId, session.lastSeq);
    this.subscribedThreads.add(session.id);
    await session.refreshTurnQueue();
  }

  private requireActiveWorkspace() {
    if (!this.activeWorkspaceValue) {
      throw new DockClientError('workspace_required', 'Select an authorized Workspace first');
    }
    return this.activeWorkspaceValue;
  }
}

function isMissingThread(error: unknown) {
  return error instanceof DockClientError
    && ['thread_not_found', 'workspace_not_allowed'].includes(error.code);
}

function requiredText(value: unknown, field: string) {
  const result = text(value);
  if (!result) throw new DockClientError('thread_not_found', `${field} is required`);
  return result;
}

function text(value: unknown) {
  return String(value || '').trim();
}

function lifecycleNotification(notification: RuntimeNotification) {
  if (!['thread/renamed', 'thread/archived', 'thread/restored', 'thread/deleted'].includes(notification.method)) {
    return undefined;
  }
  const raw = notification.params.thread;
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return undefined;
  const value = raw as Record<string, unknown>;
  const id = text(value.id);
  const workspaceId = text(value.workspaceId);
  if (!id || !workspaceId) return undefined;
  return {
    method: notification.method as ThreadLifecycleMethod,
    thread: {
      id,
      workspaceId,
      title: text(value.title) || 'New chat',
      createdAt: Number(value.createdAt) || 0,
      updatedAt: Number(value.updatedAt) || 0,
      archivedAt: Number(value.archivedAt) || undefined
    } satisfies ThreadSummary
  };
}

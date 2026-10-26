import type { DockClient, CreateThreadOptions } from '../client/DockClient.js';
import { ThreadAttentionStore } from '../client/ThreadAttentionStore.js';
import type { StorageLike } from '../client/ThreadStore.js';
import type { ThreadLifecycleEvent, ThreadLifecycleListener } from '../client/SessionCoordinator.js';
import type { ThreadSummary } from '../protocol/responses.js';
import type { AgentSession } from '../session/AgentSession.js';
import type { SessionState } from '../session/SessionState.js';

export type ThreadAttention = 'approval' | 'error' | 'completed';

export interface ThreadWorkspaceItem extends ThreadSummary {
  active: boolean;
  activity: SessionState['activity'];
  attention?: ThreadAttention;
  canArchive: boolean;
}

export interface ArchivedThreadWorkspaceItem extends ThreadSummary {
  confirmingDelete: boolean;
}

export interface ThreadWorkspaceView {
  threads: readonly ThreadWorkspaceItem[];
  archivedThreads: readonly ArchivedThreadWorkspaceItem[];
  activeThreadId?: string;
  activeTitle: string;
  attentionCount: number;
  loading: boolean;
  operation?: string;
  renameThreadId?: string;
  renameDraft: string;
  error?: string;
}

export interface ThreadWorkspaceClient {
  readonly application: string;
  readonly activeWorkspaceId: string;
  readonly activeSession: AgentSession | undefined;
  listThreads(
    workspaceId?: string,
    options?: { includeArchived?: boolean }
  ): Promise<ThreadSummary[]>;
  createThread(options?: CreateThreadOptions): Promise<AgentSession>;
  observeThread(threadId: string, workspaceId?: string): Promise<AgentSession>;
  switchThread(threadId: string, workspaceId?: string): Promise<AgentSession>;
  renameThread(threadId: string, title: string, workspaceId?: string): Promise<ThreadSummary>;
  archiveThread(threadId: string, workspaceId?: string): Promise<ThreadSummary>;
  restoreArchivedThread(threadId: string, workspaceId?: string): Promise<ThreadSummary>;
  deleteThread(
    threadId: string,
    workspaceId?: string,
    confirmation?: string
  ): Promise<{ ok: true; thread: ThreadSummary }>;
  onThreadLifecycleChange(listener: ThreadLifecycleListener): () => void;
  getSession(threadId: string): AgentSession | undefined;
}

export class ThreadController {
  private client?: ThreadWorkspaceClient;
  private attentionStore?: ThreadAttentionStore;
  private summaries: ThreadSummary[] = [];
  private readonly attention = new Map<string, ThreadAttention>();
  private readonly removeSessionListeners = new Map<string, () => void>();
  private removeLifecycleListener?: () => void;
  private loading = false;
  private operation = '';
  private error = '';
  private renameThreadId = '';
  private renameDraft = '';
  private confirmDeleteThreadId = '';
  private loadGeneration = 0;
  private titleRefresh?: Promise<void>;

  constructor(
    private readonly onChange: () => void,
    private readonly onActiveSession: (session: AgentSession | undefined) => void,
    private readonly storage?: StorageLike
  ) {}

  get view(): ThreadWorkspaceView {
    const activeThreadId = this.client?.activeSession?.id;
    const threads = this.summaries.filter((thread) => !thread.archivedAt).map((thread) => {
      const state = this.client?.getSession(thread.id)?.state;
      return {
        ...thread,
        // 还没说过话的会话：网关给的是占位标题「当前会话」，显示成「新对话」（同桌面端）。
        title: isPlaceholderTitle(thread.title) ? '新对话' : thread.title,
        active: thread.id === activeThreadId,
        activity: state?.activity || 'idle',
        attention: this.attention.get(thread.id),
        canArchive: state ? !hasPendingWork(state) : true
      };
    });
    return {
      threads,
      archivedThreads: this.summaries.filter((thread) => thread.archivedAt).map((thread) => ({
        ...thread,
        confirmingDelete: thread.id === this.confirmDeleteThreadId
      })),
      activeThreadId,
      activeTitle: threads.find((thread) => thread.active)?.title || 'Dock',
      attentionCount: threads.filter((thread) => thread.attention).length,
      loading: this.loading,
      operation: this.operation || undefined,
      renameThreadId: this.renameThreadId || undefined,
      renameDraft: this.renameDraft,
      error: this.error || undefined
    };
  }

  bind(client: DockClient | ThreadWorkspaceClient) {
    if (this.client === client) return;
    this.destroy();
    this.client = client;
    this.attentionStore = new ThreadAttentionStore(client.application, this.storage);
    this.removeLifecycleListener = client.onThreadLifecycleChange((event) => this.lifecycleChanged(event));
  }

  async load() {
    const client = this.requireClient();
    const workspaceId = client.activeWorkspaceId;
    if (!workspaceId) return;
    const generation = ++this.loadGeneration;
    this.loading = true;
    this.error = '';
    this.onChange();
    try {
      const summaries = await client.listThreads(workspaceId, { includeArchived: true });
      if (generation !== this.loadGeneration) return;
      this.summaries = summaries;
      const live = this.summaries.filter((thread) => !thread.archivedAt);
      this.pruneWatchers(new Set(live.map((thread) => thread.id)));
      await Promise.all(live.map(async (thread) => {
        const session = client.getSession(thread.id)
          || await client.observeThread(thread.id, workspaceId);
        if (generation === this.loadGeneration) this.watch(session);
      }));
      if (generation !== this.loadGeneration) return;
      const active = client.activeSession;
      if (active && this.summaries.some((thread) => thread.id === active.id)) {
        this.markActive(active);
        this.onActiveSession(active);
      }
    } catch {
      if (generation === this.loadGeneration) this.error = '会话列表未能加载，请检查连接后重试';
    } finally {
      if (generation === this.loadGeneration) {
        this.loading = false;
        this.onChange();
      }
    }
  }

  async ensureActive() {
    const client = this.requireClient();
    const active = client.activeSession;
    if (active && !active.state.thread.archivedAt) {
      this.watch(active);
      this.markActive(active);
      this.onActiveSession(active);
      return active;
    }
    const live = this.summaries.find((thread) => !thread.archivedAt);
    if (live) return this.switchTo(live.id);
    return this.create();
  }

  async create() {
    const client = this.requireClient();
    return this.run('create', async () => {
      const session = await client.createThread({
        workspaceId: client.activeWorkspaceId
      });
      // 新会话可能沿用旧 id（网关的 `live`）：旧会话对象上的监听摘掉，改听新的。
      this.detachThread(session.id);
      this.watch(session);
      this.onActiveSession(session);
      await this.load();
      this.markActive(session);
      return session;
    });
  }

  async switchTo(threadId: string) {
    const client = this.requireClient();
    if (client.activeSession?.id === threadId) {
      this.markActive(client.activeSession);
      return client.activeSession;
    }
    return this.run(`switch:${threadId}`, async () => {
      const session = await client.switchThread(threadId, client.activeWorkspaceId);
      this.watch(session);
      this.markActive(session);
      this.onActiveSession(session);
      return session;
    });
  }

  async archive(threadId: string) {
    const client = this.requireClient();
    const session = client.getSession(threadId);
    if (session && hasPendingWork(session.state)) {
      this.error = '活动中或有排队消息的会话不能归档';
      this.onChange();
      return false;
    }
    return this.run(`archive:${threadId}`, async () => {
      const wasActive = client.activeSession?.id === threadId;
      const archived = await client.archiveThread(threadId, client.activeWorkspaceId);
      this.replaceSummary(archived);
      this.detachThread(threadId);
      if (wasActive) await this.activateFallback();
      this.onChange();
      return true;
    });
  }

  beginRename(threadId: string) {
    const thread = this.summaries.find((item) => item.id === threadId && !item.archivedAt);
    if (!thread) return;
    this.renameThreadId = threadId;
    this.renameDraft = thread.title;
    this.confirmDeleteThreadId = '';
    this.error = '';
    this.onChange();
  }

  setRenameDraft(value: string) {
    if (!this.renameThreadId) return;
    this.renameDraft = value.slice(0, 160);
    this.onChange();
  }

  cancelRename() {
    if (!this.renameThreadId) return;
    this.renameThreadId = '';
    this.renameDraft = '';
    this.onChange();
  }

  async saveRename(threadId: string) {
    const client = this.requireClient();
    if (this.renameThreadId !== threadId) return false;
    const title = this.renameDraft.trim();
    if (!title) {
      this.error = '会话标题不能为空';
      this.onChange();
      return false;
    }
    return this.run(`rename:${threadId}`, async () => {
      const renamed = await client.renameThread(threadId, title, client.activeWorkspaceId);
      this.replaceSummary(renamed);
      this.renameThreadId = '';
      this.renameDraft = '';
      this.onChange();
      return true;
    });
  }

  async restoreArchived(threadId: string) {
    const client = this.requireClient();
    return this.run(`restore:${threadId}`, async () => {
      const restored = await client.restoreArchivedThread(threadId, client.activeWorkspaceId);
      this.confirmDeleteThreadId = '';
      if (restored.id === threadId) {
        this.replaceSummary(restored);
        this.onChange();
        return restored;
      }
      // 网关把它换回了第 1 页（`live`），原来的第 1 页归档成了新 id：重拉列表，切到换上来的那份。
      this.detachThread(restored.id);
      await this.load();
      const session = await client.switchThread(restored.id, client.activeWorkspaceId);
      this.watch(session);
      this.markActive(session);
      this.onActiveSession(session);
      this.onChange();
      return restored;
    });
  }

  requestDelete(threadId: string) {
    if (!this.summaries.some((thread) => thread.id === threadId && thread.archivedAt)) return;
    this.confirmDeleteThreadId = threadId;
    this.error = '';
    this.onChange();
  }

  cancelDelete() {
    if (!this.confirmDeleteThreadId) return;
    this.confirmDeleteThreadId = '';
    this.onChange();
  }

  async deletePermanently(threadId: string) {
    const client = this.requireClient();
    if (this.confirmDeleteThreadId !== threadId) {
      this.error = '请先确认永久删除这个会话';
      this.onChange();
      return false;
    }
    return this.run(`delete:${threadId}`, async () => {
      await client.deleteThread(threadId, client.activeWorkspaceId, threadId);
      this.deleteSummary(threadId);
      this.confirmDeleteThreadId = '';
      this.onChange();
      return true;
    });
  }

  clearError() {
    if (!this.error) return;
    this.error = '';
    this.onChange();
  }

  destroy() {
    this.loadGeneration += 1;
    this.removeLifecycleListener?.();
    this.removeLifecycleListener = undefined;
    for (const remove of this.removeSessionListeners.values()) remove();
    this.removeSessionListeners.clear();
    this.attention.clear();
    this.summaries = [];
    this.client = undefined;
    this.attentionStore = undefined;
    this.loading = false;
    this.operation = '';
    this.error = '';
    this.renameThreadId = '';
    this.renameDraft = '';
    this.confirmDeleteThreadId = '';
    this.tail = Promise.resolve();
  }

  private watch(session: AgentSession) {
    if (this.removeSessionListeners.has(session.id)) return;
    const stored = this.attentionStore?.seenSeq(session.id);
    if (this.client?.activeSession?.id === session.id || stored === undefined) {
      this.attentionStore?.markSeen(session.id, session.lastSeq);
    } else {
      this.updateAttention(session.state, stored);
    }
    const remove = session.onChange((state) => this.sessionChanged(state));
    this.removeSessionListeners.set(session.id, remove);
  }

  private sessionChanged(state: Readonly<SessionState>) {
    this.summaries = this.summaries
      .map((thread) => thread.id === state.thread.id ? {
        ...thread,
        title: keptTitle(state, thread.title),
        updatedAt: state.thread.updatedAt
      } : thread)
      .sort((left, right) => right.updatedAt - left.updatedAt);
    if (this.client?.activeSession?.id === state.thread.id) this.refreshTitleAfterTurn(state);
    if (this.client?.activeSession?.id === state.thread.id) {
      this.attentionStore?.markSeen(state.thread.id, state.lastSeq);
      this.attention.delete(state.thread.id);
    } else {
      const seenSeq = this.attentionStore?.seenSeq(state.thread.id);
      if (seenSeq !== undefined) this.updateAttention(state, seenSeq);
    }
    this.onChange();
  }

  /**
   * 网关的 `live` 会话标题取自第一条用户消息，只在列表里现算，不推送改名：
   * 这一轮说完了、标题还是占位的，就重拉一次列表，只更新标题。
   */
  private refreshTitleAfterTurn(state: Readonly<SessionState>) {
    const summary = this.summaries.find((thread) => thread.id === state.thread.id);
    if (!summary || !isPlaceholderTitle(summary.title)) return;
    if (!state.messages.some((message) => message.role === 'user')) return;
    if (hasPendingWork(state) || this.titleRefresh) return;
    const client = this.client;
    const workspaceId = client?.activeWorkspaceId;
    if (!client || !workspaceId) return;
    this.titleRefresh = client.listThreads(workspaceId, { includeArchived: false })
      .then((listed) => {
        const titles = new Map(listed.map((thread) => [thread.id, thread.title]));
        let changed = false;
        this.summaries = this.summaries.map((thread) => {
          const title = titles.get(thread.id);
          if (!title || isPlaceholderTitle(title) || title === thread.title) return thread;
          changed = true;
          return { ...thread, title };
        });
        if (changed) this.onChange();
      })
      .catch(() => undefined)
      .finally(() => {
        this.titleRefresh = undefined;
      });
  }

  private lifecycleChanged(event: Readonly<ThreadLifecycleEvent>) {
    if (event.source === 'local') return;
    if (event.method === 'thread/deleted') {
      this.deleteSummary(event.thread.id);
    } else {
      this.replaceSummary(event.thread);
      if (event.method === 'thread/archived') this.detachThread(event.thread.id);
    }
    this.onChange();
    if (event.wasActive && (
      event.method === 'thread/archived' || event.method === 'thread/deleted'
    )) {
      void this.activateFallback().catch(() => {
        this.error = '归档或删除已同步，但没有可用的活动会话';
        this.onActiveSession(undefined);
        this.onChange();
      });
    }
  }

  private updateAttention(state: Readonly<SessionState>, seenSeq: number) {
    if (state.lastSeq <= seenSeq) {
      this.attention.delete(state.thread.id);
      return;
    }
    const attention = attentionFromState(state);
    if (attention) this.attention.set(state.thread.id, attention);
    else this.attention.delete(state.thread.id);
  }

  private markActive(session: AgentSession) {
    this.attentionStore?.markSeen(session.id, session.lastSeq);
    this.attention.delete(session.id);
    this.onChange();
  }

  private deleteSummary(threadId: string) {
    this.summaries = this.summaries.filter((thread) => thread.id !== threadId);
    if (this.confirmDeleteThreadId === threadId) this.confirmDeleteThreadId = '';
    this.detachThread(threadId);
  }

  private detachThread(threadId: string) {
    this.removeSessionListeners.get(threadId)?.();
    this.removeSessionListeners.delete(threadId);
    this.attention.delete(threadId);
    this.attentionStore?.clear(threadId);
    if (this.renameThreadId === threadId) {
      this.renameThreadId = '';
      this.renameDraft = '';
    }
  }

  private replaceSummary(thread: ThreadSummary) {
    this.summaries = this.summaries.some((item) => item.id === thread.id)
      ? this.summaries.map((item) => item.id === thread.id ? thread : item)
      : [...this.summaries, thread];
    this.summaries.sort((left, right) => right.updatedAt - left.updatedAt);
  }

  private async activateFallback() {
    const client = this.requireClient();
    const next = this.summaries.find((thread) => !thread.archivedAt);
    const active = next
      ? await client.switchThread(next.id, client.activeWorkspaceId)
      : await client.createThread({ workspaceId: client.activeWorkspaceId });
    this.replaceSummary(active.state.thread);
    this.watch(active);
    this.markActive(active);
    this.onActiveSession(active);
    return active;
  }

  private pruneWatchers(keep: ReadonlySet<string>) {
    for (const [threadId, remove] of this.removeSessionListeners) {
      if (keep.has(threadId)) continue;
      remove();
      this.removeSessionListeners.delete(threadId);
      this.attention.delete(threadId);
    }
  }

  private tail: Promise<void> = Promise.resolve();

  private run<T>(operation: string, action: () => Promise<T>): Promise<T> {
    const started = this.tail.then(async () => {
      this.operation = operation;
      this.error = '';
      this.onChange();
      try {
        return await action();
      } catch (error) {
        this.error = '会话操作未完成，请检查连接后重试';
        throw error;
      } finally {
        this.operation = '';
        this.onChange();
      }
    });
    this.tail = started.then(() => undefined, () => undefined);
    return started;
  }

  private requireClient() {
    if (!this.client) throw new Error('Thread workspace is not connected');
    return this.client;
  }
}

export function attentionFromState(state: Readonly<SessionState>): ThreadAttention | undefined {
  if (state.activity === 'error') return 'error';
  if (state.permissionRequests.some((request) => request.status === 'pending') || state.activity === 'approval') {
    return 'approval';
  }
  return state.activity === 'success' ? 'completed' : undefined;
}

/**
 * 会话里还是占位标题、但已经说过话：留着列表里拿到的真标题（网关只在列表里现算）。
 * 没说过话的占位标题就是真的（开了新会话、网关重启后的空会话），不能留着旧标题。
 */
function keptTitle(state: Readonly<SessionState>, listed: string) {
  if (!isPlaceholderTitle(state.thread.title)) return state.thread.title;
  return state.messages.some((message) => message.role === 'user') ? listed : state.thread.title;
}

/** 网关给还没说过话的会话的占位标题。 */
const PLACEHOLDER_TITLE = '当前会话';

function isPlaceholderTitle(title: string | undefined) {
  return !title || title === PLACEHOLDER_TITLE;
}

export function hasPendingWork(state: Readonly<SessionState>) {
  return state.turnQueue.length > 0 || Object.values(state.turns).some((turn) =>
    turn.status === 'running' || turn.status === 'queued' || turn.status === 'waiting_permission'
  );
}

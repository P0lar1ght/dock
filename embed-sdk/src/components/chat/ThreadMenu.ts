import { html, nothing } from 'lit';
import type {
  ArchivedThreadWorkspaceItem,
  ThreadWorkspaceItem,
  ThreadWorkspaceView
} from '../../controllers/ThreadController.js';

export interface ThreadMenuActions {
  createThread: () => void;
  switchThread: (threadId: string) => void;
  beginRenameThread: (threadId: string) => void;
  renameDraft: (value: string) => void;
  saveRenameThread: (threadId: string) => void;
  cancelRenameThread: () => void;
  archiveThread: (threadId: string) => void;
  restoreArchivedThread: (threadId: string) => void;
  requestDeleteThread: (threadId: string) => void;
  confirmDeleteThread: (threadId: string) => void;
  cancelDeleteThread: () => void;
}

export function threadMenu(model: ThreadWorkspaceView, actions: ThreadMenuActions) {
  return html`
    <details class="thread-menu" data-testid="thread-menu">
      <summary class="icon-button thread-menu-toggle" aria-label="Threads">
        <span aria-hidden="true">#</span>
        ${model.attentionCount
          ? html`<span class="thread-menu-count" data-testid="thread-attention-count">${model.attentionCount}</span>`
          : nothing}
      </summary>
      <div class="thread-menu-popover">
        <div class="thread-menu-heading">
          <strong>Threads</strong>
          <button
            class="thread-new-button"
            data-testid="thread-new"
            type="button"
            ?disabled=${Boolean(model.operation)}
            @click=${(event: Event) => {
              closeMenu(event);
              actions.createThread();
            }}
          >＋ New</button>
        </div>
        <div class="thread-list" role="list" aria-busy=${String(model.loading)}>
          ${model.threads.length
            ? model.threads.map((thread) => threadRow(thread, model, actions))
            : html`<p class="thread-empty">${model.loading ? 'Loading…' : 'No Threads yet'}</p>`}
          ${archivedThreads(model, actions)}
        </div>
        ${model.error ? html`<p class="thread-menu-error" role="alert">${model.error}</p>` : nothing}
      </div>
    </details>
  `;
}

function archivedThreads(model: ThreadWorkspaceView, actions: ThreadMenuActions) {
  return html`
    <details class="thread-archived-section" data-testid="thread-archived-section">
      <summary>已归档 <span>${model.archivedThreads.length}</span></summary>
      <div class="thread-archived-list" role="list">
        ${model.archivedThreads.length
          ? model.archivedThreads.map((thread) => archivedThreadRow(thread, model, actions))
          : html`<p class="thread-empty">没有已归档的 Thread</p>`}
      </div>
    </details>
  `;
}

function archivedThreadRow(
  thread: ArchivedThreadWorkspaceItem,
  model: ThreadWorkspaceView,
  actions: ThreadMenuActions
) {
  return html`
    <div class="thread-row thread-row-archived" data-testid="thread-archived-row" data-thread-id=${thread.id} role="listitem">
      <span class="thread-archived-copy">
        <span class="thread-title">${thread.title}</span>
        <span class="thread-meta">已归档</span>
      </span>
      <span class="thread-row-actions">
        ${thread.confirmingDelete ? html`
          <button
            class="thread-action"
            type="button"
            ?disabled=${Boolean(model.operation)}
            @click=${actions.cancelDeleteThread}
          >取消</button>
          <button
            class="thread-action thread-delete-confirm"
            data-testid="thread-delete-confirm"
            type="button"
            aria-label=${`Permanently delete ${thread.title}`}
            ?disabled=${Boolean(model.operation)}
            @click=${() => actions.confirmDeleteThread(thread.id)}
          >确认删除</button>
        ` : html`
          <button
            class="thread-action"
            type="button"
            ?disabled=${Boolean(model.operation)}
            @click=${(event: Event) => {
              closeMenu(event);
              actions.restoreArchivedThread(thread.id);
            }}
          >恢复</button>
        `}
      </span>
    </div>
  `;
}

function threadRow(
  thread: ThreadWorkspaceItem,
  model: ThreadWorkspaceView,
  actions: ThreadMenuActions
) {
  const renaming = model.renameThreadId === thread.id;
  return html`
    <div
      class="thread-row"
      data-active=${String(thread.active)}
      data-testid="thread-row"
      data-thread-id=${thread.id}
      role="listitem"
    >
      ${renaming ? renameEditor(thread, model, actions) : html`
        <button
          class="thread-switch"
          type="button"
          aria-current=${thread.active ? 'true' : 'false'}
          @click=${(event: Event) => {
            closeMenu(event);
            actions.switchThread(thread.id);
          }}
        >
          <span class="thread-state" data-activity=${thread.activity} data-attention=${thread.attention || ''}></span>
          <span class="thread-copy">
            <span class="thread-title">${thread.title}</span>
            <span class="thread-meta">${threadStatus(thread)}</span>
          </span>
        </button>
      `}
    </div>
  `;
}

function renameEditor(
  thread: ThreadWorkspaceItem,
  model: ThreadWorkspaceView,
  actions: ThreadMenuActions
) {
  return html`
    <div class="thread-rename-editor">
      <input
        data-testid="thread-rename-input"
        aria-label=${`Rename ${thread.title}`}
        .value=${model.renameDraft}
        maxlength="160"
        autofocus
        ?disabled=${Boolean(model.operation)}
        @input=${(event: InputEvent) => actions.renameDraft((event.currentTarget as HTMLInputElement).value)}
        @keydown=${(event: KeyboardEvent) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            actions.saveRenameThread(thread.id);
          } else if (event.key === 'Escape') {
            event.preventDefault();
            actions.cancelRenameThread();
          }
        }}
      />
      <button
        class="thread-action"
        type="button"
        ?disabled=${Boolean(model.operation)}
        @click=${actions.cancelRenameThread}
      >取消</button>
      <button
        class="thread-action thread-rename-confirm"
        data-testid="thread-rename-confirm"
        type="button"
        ?disabled=${Boolean(model.operation) || !model.renameDraft.trim()}
        @click=${() => actions.saveRenameThread(thread.id)}
      >保存</button>
    </div>
  `;
}

function threadStatus(thread: ThreadWorkspaceItem) {
  if (thread.attention === 'approval') return '需要审批';
  if (thread.attention === 'error') return '运行失败';
  if (thread.attention === 'completed') return '已完成';
  const labels: Record<ThreadWorkspaceItem['activity'], string> = {
    idle: thread.active ? '当前' : '就绪',
    thinking: '思考中',
    streaming: '回复中',
    working: '工具运行中',
    approval: '等待审批',
    input: '等待选择',
    subagent: 'SubAgent 运行中',
    success: '已完成',
    error: '失败'
  };
  return labels[thread.activity];
}

function closeMenu(event: Event) {
  (event.currentTarget as HTMLElement).closest('details')?.removeAttribute('open');
}

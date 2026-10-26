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
      <summary class="icon-button thread-menu-toggle" aria-label="会话">
        <span aria-hidden="true">#</span>
        ${model.attentionCount
          ? html`<span class="thread-menu-count" data-testid="thread-attention-count">${model.attentionCount}</span>`
          : nothing}
      </summary>
      <div class="thread-menu-popover">
        <div class="thread-menu-heading">
          <strong>会话</strong>
          <button
            class="thread-new-button"
            data-testid="thread-new"
            type="button"
            ?disabled=${Boolean(model.operation)}
            @click=${(event: Event) => {
              closeMenu(event);
              actions.createThread();
            }}
          >＋ 新会话</button>
        </div>
        <div class="thread-list" role="list" aria-busy=${String(model.loading)}>
          ${model.threads.length
            ? model.threads.map((thread) => threadRow(thread, model, actions))
            : html`<p class="thread-empty">${model.loading ? '加载中…' : '还没有会话'}</p>`}
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
          : html`<p class="thread-empty">没有已归档的会话</p>`}
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
            aria-label=${`确认永久删除 ${thread.title}`}
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
          <button
            class="thread-action thread-delete"
            data-testid="thread-delete"
            type="button"
            aria-label=${`永久删除 ${thread.title}`}
            ?disabled=${Boolean(model.operation)}
            @click=${() => actions.requestDeleteThread(thread.id)}
          >删除</button>
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
        <span class="thread-row-actions">
          <button
            class="thread-rename"
            data-testid="thread-rename"
            type="button"
            title="重命名"
            aria-label=${`重命名 ${thread.title}`}
            ?disabled=${Boolean(model.operation)}
            @click=${() => actions.beginRenameThread(thread.id)}
          >✎</button>
          <button
            class="thread-action"
            data-testid="thread-archive"
            type="button"
            title=${thread.canArchive ? '归档' : '还有排队的消息，先发出或删掉再归档'}
            aria-label=${`归档 ${thread.title}`}
            ?disabled=${Boolean(model.operation) || !thread.canArchive}
            @click=${() => actions.archiveThread(thread.id)}
          >归档</button>
        </span>
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
        aria-label=${`重命名 ${thread.title}`}
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
    subagent: '子代理运行中',
    // 后台跑完的会话由上面的 attention「已完成」标出来；会话里 success 会一直留着，不能一直显示。
    success: thread.active ? '当前' : '就绪',
    error: '失败'
  };
  return labels[thread.activity];
}

function closeMenu(event: Event) {
  (event.currentTarget as HTMLElement).closest('details')?.removeAttribute('open');
}

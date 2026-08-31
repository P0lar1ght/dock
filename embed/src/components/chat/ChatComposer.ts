import { html, nothing } from 'lit';
import { faArrowUp } from '@fortawesome/free-solid-svg-icons/faArrowUp';
import { faChevronDown } from '@fortawesome/free-solid-svg-icons/faChevronDown';
import { faHand } from '@fortawesome/free-solid-svg-icons/faHand';
import { faPlus } from '@fortawesome/free-solid-svg-icons/faPlus';
import { faPaperclip } from '@fortawesome/free-solid-svg-icons/faPaperclip';
import { faArrowLeft } from '@fortawesome/free-solid-svg-icons/faArrowLeft';
import { faArrowRight } from '@fortawesome/free-solid-svg-icons/faArrowRight';
import { faXmark } from '@fortawesome/free-solid-svg-icons/faXmark';
import type { ActiveTurnSendMode } from '../../controllers/ChatController.js';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';
import { contextUsagePopover } from './ContextUsagePopover.js';
import { modelSelectionPopover } from './ModelSelectionPopover.js';
import { reasoningSelectionPopover } from './ReasoningSelectionPopover.js';
import { approvalModePopover } from './ApprovalModePopover.js';
import { goalPopover } from './GoalPopover.js';
import { planModePopover } from './PlanModePopover.js';
import { memoryControlPopover } from './MemoryControlPopover.js';
import type { ThreadMemorySelection } from '../../controllers/ThreadExecutionControlController.js';
import type { ApprovalMode, ReasoningEffort } from '../../protocol/responses.js';
import type { SlashCommandSuggestion } from '../../controllers/SlashCommandModel.js';
import type { PendingImageInput } from '../../image-inputs/PendingImageInputStore.js';
import type { TurnIntentMode } from '../../controllers/ChatViewState.js';

export interface ChatComposerModel {
  draft: string;
  pendingImages: readonly PendingImageInput[];
  sessionReady: boolean;
  activeTurn: boolean;
  sendMode: ActiveTurnSendMode;
  turnIntent: TurnIntentMode;
  submitting: boolean;
  capturingScreenshot: boolean;
  canSend: boolean;
  slashCommands: readonly SlashCommandSuggestion[];
  slashCommandIndex: number;
  menu?: 'context' | 'model' | 'reasoning' | 'approval' | 'goal' | 'plan' | 'memory';
  modelChanging: boolean;
  executionChanging: boolean;
  approvalConfirmationPending: boolean;
  goalDraft: string;
  goalChanging: boolean;
  environment?: SessionEnvironment;
  error?: string;
}

export interface ChatComposerActions {
  draft: (value: string) => void;
  mode: (mode: ActiveTurnSendMode) => void;
  send: () => void;
  toggleMenu: () => void;
  closeMenu: () => void;
  toggleModel: () => void;
  openModel: () => void;
  selectModel: (modelId: string) => void;
  refreshModels: () => void;
  openReasoning: () => void;
  selectReasoning: (effort: ReasoningEffort) => void;
  toggleApproval: () => void;
  selectApproval: (mode: ApprovalMode) => void;
  openGoal: () => void;
  goalDraft: (value: string) => void;
  saveGoal: () => void;
  pauseGoal: () => void;
  resumeGoal: () => void;
  clearGoal: () => void;
  openPlan: () => void;
  selectPlanMode: (enabled: boolean) => void;
  openMemory: () => void;
  selectMemory: (selection: ThreadMemorySelection) => void;
  compactContext: () => void;
  moveSlashCommand: (delta: number) => void;
  completeSlashCommand: (index?: number) => void;
  addImages: (files: readonly File[]) => void;
  removeImage: (id: string) => void;
  moveImage: (id: string, delta: number) => void;
}

export function chatComposer(model: ChatComposerModel, actions: ChatComposerActions) {
  const placeholder = model.sessionReady
    ? model.submitting
      ? model.capturingScreenshot ? '正在截取当前界面…' : '正在提交…'
      : model.activeTurn
        ? model.sendMode === 'steer' ? '输入对当前回复的引导…' : '输入稍后处理的消息…'
        : '给嘟嘟发消息…'
    : '本地 Agent 尚未连接';
  const sendLabel = model.activeTurn
    ? model.sendMode === 'steer' ? '引导当前回复' : '加入消息队列'
    : '发送消息';
  const approval = approvalLabel(model.environment?.approval.mode);
  const modelLabel = model.environment?.model.label || '模型';
  const imageSupported = model.environment?.model.inputModalities?.includes('image') === true;
  return html`
    <footer class="composer-shell">
      ${model.error ? html`<p class="composer-error" role="alert">${model.error}</p>` : ''}
      ${model.activeTurn ? activeSendModes(model.sendMode, actions) : nothing}
      ${model.slashCommands.length ? slashCommandMenu(model, actions) : nothing}
      ${model.menu ? html`
        <button
          class="composer-popover-scrim"
          data-testid="composer-popover-scrim"
          type="button"
          aria-label="关闭 Composer 菜单"
          @click=${actions.closeMenu}
        ></button>
        ${model.menu === 'model'
          ? modelSelectionPopover(
            model.environment,
            model.activeTurn,
            model.modelChanging,
            actions.selectModel,
            actions.refreshModels,
            actions.toggleMenu
          )
          : model.menu === 'reasoning'
            ? reasoningSelectionPopover(
              model.environment,
              model.activeTurn,
              model.executionChanging,
              actions.selectReasoning,
              actions.toggleMenu
            )
            : model.menu === 'approval'
              ? approvalModePopover(
                model.environment,
                model.activeTurn,
                model.executionChanging,
                model.approvalConfirmationPending,
                actions.selectApproval
              )
              : model.menu === 'goal'
                ? goalPopover(
                  model.environment,
                  model.goalDraft,
                  model.goalChanging,
                  actions.goalDraft,
                  actions.saveGoal,
                  actions.pauseGoal,
                  actions.resumeGoal,
                  actions.clearGoal
                )
                : model.menu === 'plan'
                  ? planModePopover(
                    model.environment,
                    model.activeTurn,
                    model.executionChanging,
                    model.turnIntent,
                    actions.selectPlanMode
                  )
                  : model.menu === 'memory'
                    ? memoryControlPopover(
                      model.environment,
                      model.activeTurn,
                      model.executionChanging,
                      actions.selectMemory
                    )
                    : contextUsagePopover(
                      model.environment,
                      !model.sessionReady,
                      actions.openModel,
                      actions.openReasoning,
                      actions.openGoal,
                      actions.openPlan,
                      actions.openMemory,
                      model.activeTurn,
                      model.executionChanging,
                      actions.compactContext
                    )}
      ` : nothing}
      <form
        class="composer"
        @submit=${(event: SubmitEvent) => submit(event, model, actions)}
        @dragover=${dragOver}
        @drop=${(event: DragEvent) => dropImages(event, actions)}
      >
        ${model.pendingImages.length ? pendingImageStrip(model.pendingImages, actions) : nothing}
        <textarea
          data-testid="chat-input"
          aria-label="Message"
          rows="1"
          maxlength="12000"
          .value=${model.draft}
          placeholder=${placeholder}
          ?disabled=${!model.sessionReady || model.submitting}
          @input=${(event: InputEvent) => actions.draft((event.currentTarget as HTMLTextAreaElement).value)}
          @keydown=${(event: KeyboardEvent) => keyDown(event, model, actions)}
          @paste=${(event: ClipboardEvent) => pasteImages(event, actions)}
        ></textarea>
        <div class="composer-toolbar">
          <div class="composer-toolbar-start">
            <button
              class="composer-control composer-plus"
              data-testid="context-menu-toggle"
              type="button"
              aria-label="查看上下文用量"
              aria-expanded=${String(model.menu === 'context')}
              @click=${actions.toggleMenu}
            >${iconTemplate(faPlus)}</button>
            <button
              class="composer-control attachment-control"
              data-testid="image-file-trigger"
              type="button"
              aria-label="添加图片"
              ?disabled=${!model.sessionReady || !imageSupported || model.submitting || model.pendingImages.length >= 4}
              @click=${(event: Event) => {
                const form = (event.currentTarget as HTMLElement).closest('form');
                form?.querySelector<HTMLInputElement>('[data-testid="image-file-input"]')?.click();
              }}
            >${iconTemplate(faPaperclip)}</button>
            <button
              class="composer-control approval-control"
              type="button"
              aria-label=${`审批模式：${approval}`}
              aria-expanded=${String(model.menu === 'approval')}
              ?disabled=${!model.environment?.approval.canChange || model.executionChanging}
              @click=${actions.toggleApproval}
            >
              ${iconTemplate(faHand)}
            </button>
          </div>
          <div class="composer-toolbar-end">
            <button
              class="composer-control model-control"
              type="button"
              aria-label=${`模型：${modelLabel}`}
              aria-expanded=${String(model.menu === 'model')}
              ?disabled=${!model.environment?.model.canChange || model.modelChanging}
              @click=${actions.toggleModel}
            >
              <span>${modelLabel}</span>
              ${iconTemplate(faChevronDown, 'control-icon control-chevron')}
            </button>
            <button
              class="send-button"
              data-testid="chat-send"
              type="submit"
              aria-label=${sendLabel}
              ?disabled=${!model.canSend}
            >${iconTemplate(faArrowUp)}</button>
          </div>
        </div>
        <input
          data-testid="image-file-input"
          type="file"
          accept="image/png,image/jpeg,image/webp"
          multiple
          hidden
          @change=${(event: Event) => {
            const input = event.currentTarget as HTMLInputElement;
            actions.addImages(Array.from(input.files || []));
            input.value = '';
          }}
        />
      </form>
      <p class="composer-hint">${model.activeTurn ? `Enter ${sendLabel}` : 'Enter 发送'} · Shift + Enter 换行</p>
    </footer>
  `;
}

function pendingImageStrip(
  images: readonly PendingImageInput[],
  actions: ChatComposerActions
) {
  return html`
    <div class="pending-image-strip" data-testid="pending-image-strip" aria-label="待发送图片">
      ${images.map((image, index) => html`
        <figure class="pending-image">
          <img src=${image.previewUrl} alt=${image.name} />
          <figcaption title=${image.name}>${index + 1}. ${image.name}</figcaption>
          <div>
            <button
              type="button"
              aria-label=${`左移 ${image.name}`}
              ?disabled=${index === 0}
              @click=${() => actions.moveImage(image.id, -1)}
            >${iconTemplate(faArrowLeft)}</button>
            <button
              type="button"
              aria-label=${`右移 ${image.name}`}
              ?disabled=${index === images.length - 1}
              @click=${() => actions.moveImage(image.id, 1)}
            >${iconTemplate(faArrowRight)}</button>
            <button
              type="button"
              aria-label=${`移除 ${image.name}`}
              @click=${() => actions.removeImage(image.id)}
            >${iconTemplate(faXmark)}</button>
          </div>
        </figure>
      `)}
    </div>
  `;
}

function pasteImages(event: ClipboardEvent, actions: ChatComposerActions) {
  const files = Array.from(event.clipboardData?.items || [])
    .filter((item) => item.kind === 'file' && item.type.startsWith('image/'))
    .flatMap((item) => {
      const file = item.getAsFile();
      return file ? [file] : [];
    });
  if (!files.length) return;
  event.preventDefault();
  actions.addImages(files);
}

function dragOver(event: DragEvent) {
  if (!Array.from(event.dataTransfer?.items || []).some((item) =>
    item.kind === 'file' && item.type.startsWith('image/')
  )) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
}

function dropImages(event: DragEvent, actions: ChatComposerActions) {
  const files = droppedImageFiles(event.dataTransfer?.files || []);
  if (!files.length) return;
  event.preventDefault();
  actions.addImages(files);
}

export function droppedImageFiles(files: ArrayLike<File> | Iterable<File>): File[] {
  return Array.from(files).filter((file) => file.type.startsWith('image/'));
}

function slashCommandMenu(model: ChatComposerModel, actions: ChatComposerActions) {
  return html`
    <div class="slash-command-menu" data-testid="slash-command-menu" role="listbox" aria-label="斜杠命令">
      ${model.slashCommands.map((command, index) => html`
        <button
          type="button"
          role="option"
          aria-selected=${String(index === model.slashCommandIndex)}
          ?disabled=${!command.available}
          @mousedown=${(event: MouseEvent) => event.preventDefault()}
          @click=${() => actions.completeSlashCommand(index)}
        >
          <span>
            <strong>${command.name}</strong>
            <small>${command.title}</small>
          </span>
          <em>${command.unavailableReason || command.description}</em>
        </button>
      `)}
      <p>↑↓ 选择 · Tab / Enter 补全</p>
    </div>
  `;
}

function approvalLabel(value: SessionEnvironment['approval']['mode'] | undefined) {
  if (value === 'full_access') return '完全访问';
  if (value === 'auto') return '按风险审批';
  return '请求审批';
}

function activeSendModes(mode: ActiveTurnSendMode, actions: ChatComposerActions) {
  return html`
    <div class="active-send-modes" data-testid="active-send-modes" role="group" aria-label="进行中消息方式">
      <span>当前回复进行中</span>
      <button
        type="button"
        data-testid="send-mode-queue"
        aria-pressed=${String(mode === 'queue')}
        @click=${() => actions.mode('queue')}
      >排队</button>
      <button
        type="button"
        data-testid="send-mode-steer"
        aria-pressed=${String(mode === 'steer')}
        @click=${() => actions.mode('steer')}
      >引导</button>
    </div>
  `;
}

function submit(event: SubmitEvent, model: ChatComposerModel, actions: ChatComposerActions) {
  event.preventDefault();
  const command = model.slashCommands[model.slashCommandIndex];
  if (command?.available) {
    actions.completeSlashCommand();
    return;
  }
  if (model.canSend) actions.send();
}

function keyDown(event: KeyboardEvent, model: ChatComposerModel, actions: ChatComposerActions) {
  if (event.isComposing) return;
  if (model.slashCommands.length && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
    event.preventDefault();
    actions.moveSlashCommand(event.key === 'ArrowDown' ? 1 : -1);
    return;
  }
  if (model.slashCommands.length && (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey))) {
    event.preventDefault();
    if (model.slashCommands[model.slashCommandIndex]?.available) actions.completeSlashCommand();
    return;
  }
  if (event.key !== 'Enter' || event.shiftKey) return;
  event.preventDefault();
  if (model.canSend) actions.send();
}

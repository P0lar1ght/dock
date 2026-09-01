import { html, nothing } from 'lit';
import type { PetSkinSummary } from '../pet/types.js';
import type { ChatViewState } from '../controllers/ChatController.js';
import { chatComposer } from './chat/ChatComposer.js';
import { chatMessageList } from './chat/ChatMessageList.js';
import type { PermissionDecision } from '../protocol/permissions.js';
import { turnControls } from './chat/TurnControls.js';
import { connectionStatus } from './chat/ConnectionStatus.js';
import type { RecoveryView } from '../controllers/RecoveryController.js';
import type { ActiveTurnSendMode } from '../controllers/ChatController.js';
import { turnQueueStrip } from './chat/TurnQueueStrip.js';
import type { ThreadWorkspaceView } from '../controllers/ThreadController.js';
import { threadMenu } from './chat/ThreadMenu.js';
import type { SessionRuntimeIssue } from '../session/RuntimeIssueModel.js';
import { safeRuntimeIssueText } from '../session/RuntimeIssueModel.js';
import { commandOutputCard } from './chat/CommandOutputCard.js';
import { operationIssueNotice, type OperationIssueView } from './chat/OperationIssueNotice.js';
import type { ApprovalMode, ReasoningEffort } from '../protocol/responses.js';
import type { ThreadMemorySelection } from '../controllers/ThreadExecutionControlController.js';
import type { GatewayConnectionView } from '../controllers/GatewayConnectionController.js';
import type { PairingView } from '../controllers/PairingController.js';
import type { SessionUserInputRequest } from '../session/UserInputModel.js';

export interface ChatPanelModel extends ChatViewState {
  open: boolean;
  title: string;
  status: string;
  connected: boolean;
  recovery: RecoveryView;
  pairing?: PairingView;
  gateway?: GatewayConnectionView;
  threadWorkspace: ThreadWorkspaceView;
  skins: readonly PetSkinSummary[];
  warning?: string;
}

export interface ChatPanelActions {
  close: () => void;
  retry: () => void;
  refreshPairing: () => void;
  openGateway: () => void;
  gatewayDraft: (value: string) => void;
  connectGateway: () => void;
  cancelGateway: () => void;
  draft: (value: string) => void;
  sendMode: (mode: ActiveTurnSendMode) => void;
  send: () => void;
  toggleComposerMenu: () => void;
  closeComposerMenu: () => void;
  toggleModelMenu: () => void;
  openModelMenu: () => void;
  selectModel: (modelId: string) => void;
  refreshModels: () => void;
  openReasoningMenu: () => void;
  selectReasoning: (effort: ReasoningEffort) => void;
  toggleApprovalMenu: () => void;
  selectApproval: (mode: ApprovalMode) => void;
  openGoalMenu: () => void;
  goalDraft: (value: string) => void;
  saveGoal: () => void;
  pauseGoal: () => void;
  resumeGoal: () => void;
  clearGoal: () => void;
  openPlanMenu: () => void;
  selectPlanMode: (enabled: boolean) => void;
  openMemoryMenu: () => void;
  selectMemory: (selection: ThreadMemorySelection) => void;
  compactContext: () => void;
  moveSlashCommand: (delta: number) => void;
  completeSlashCommand: (index?: number) => void;
  addImages: (files: readonly File[]) => void;
  captureScreen: () => void;
  removeImage: (id: string) => void;
  moveImage: (id: string, delta: number) => void;
  removeQueuedTurn: (queueId: string) => void;
  selectSkin: (id: string) => void;
  importSkin: () => void;
  skinFile: (file: File) => void;
  resolvePermission: (requestId: string, decision: PermissionDecision) => void;
  selectUserInput: (interactionId: string, questionId: string, value: string) => void;
  setUserInputOther: (interactionId: string, questionId: string, value: string) => void;
  submitUserInput: (request: SessionUserInputRequest) => void;
  cancelTurn: () => void;
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
  retryIssue: (issue: SessionRuntimeIssue) => void;
  editIssue: (issue: SessionRuntimeIssue) => void;
  dismissIssue: (issueId: string) => void;
  retryMessage: () => void;
  clearMessageError: () => void;
  clearThreadError: () => void;
  dismissCommandOutput: () => void;
}

export function chatPanel(model: ChatPanelModel, actions: ChatPanelActions) {
  if (!model.open) return nothing;
  return html`
    <section
      id="dock-chat-panel"
      class="chat-panel"
      data-testid="chat-panel"
      role="dialog"
      aria-modal="false"
      aria-label="${model.title} Chat"
    >
      <div class="panel-content">
        <header class="panel-header">
          <div class="panel-avatar" aria-hidden="true">嘟</div>
          <div class="panel-heading">
            <h2 class="panel-title">${model.title}</h2>
            <p class="panel-subtitle">
              <span class="header-status-dot" data-live=${String(model.connected)}></span>${model.status}
            </p>
          </div>
          ${threadMenu(model.threadWorkspace, actions)}
          ${skinMenu(model, actions)}
          <button class="icon-button" type="button" aria-label="Close Chat" @click=${actions.close}>×</button>
        </header>

        ${chatMessageList({
          messages: model.messages,
          toolActivities: model.toolActivities,
          permissionRequests: model.permissionRequests,
          userInputRequests: model.userInputRequests,
          planActivities: model.planActivities,
          goalActivities: model.goalActivities,
          subagentActivities: model.subagentActivities,
          runtimeIssues: model.runtimeIssues,
          runtimeIssueInteractions: model.runtimeIssueInteractions,
          permissionInteractions: model.permissionInteractions,
          userInputInteractions: model.userInputInteractions,
          activity: model.activity,
          sessionReady: model.sessionReady
        }, actions.resolvePermission, {
          select: actions.selectUserInput,
          other: actions.setUserInputOther,
          submit: actions.submitUserInput
        }, {
          retryIssue: actions.retryIssue,
          editIssue: actions.editIssue,
          dismissIssue: actions.dismissIssue
        })}

        <div class="chat-footer">
          ${commandOutputCard(model.commandOutput, actions.dismissCommandOutput)}
          ${connectionStatus(model.recovery, model.pairing, model.gateway, {
            retry: actions.retry,
            refreshPairing: actions.refreshPairing,
            openGateway: actions.openGateway,
            gatewayDraft: actions.gatewayDraft,
            connectGateway: actions.connectGateway,
            cancelGateway: actions.cancelGateway
          })}
          ${turnControls(model.turnControl, actions.cancelTurn)}
          ${turnQueueStrip(model.turnQueue, actions.removeQueuedTurn)}
          ${operationIssueNotice(
            operationIssue(model),
            actions.retryMessage,
            model.error ? actions.clearMessageError : actions.clearThreadError
          )}
          ${chatComposer({
            draft: model.draft,
            pendingImages: model.pendingImages,
            sessionReady: model.sessionReady,
            activeTurn: model.activeTurn,
            sendMode: model.sendMode,
            turnIntent: model.turnIntent,
            submitting: model.submitting,
            capturingScreenshot: model.capturingScreenshot,
            canSend: model.canSend,
            slashCommands: model.slashCommands,
            slashCommandIndex: model.slashCommandIndex,
            menu: model.composerMenu,
            modelChanging: model.modelChanging,
            executionChanging: model.executionChanging,
            approvalConfirmationPending: model.approvalConfirmationPending,
            goalDraft: model.goalDraft,
            goalChanging: model.goalChanging,
            environment: model.environment,
            error: model.recovery.visible ? undefined : (model.error || model.warning)
          }, {
            draft: actions.draft,
            mode: actions.sendMode,
            send: actions.send,
            toggleMenu: actions.toggleComposerMenu,
            closeMenu: actions.closeComposerMenu,
            toggleModel: actions.toggleModelMenu,
            openModel: actions.openModelMenu,
            selectModel: actions.selectModel,
            refreshModels: actions.refreshModels,
            openReasoning: actions.openReasoningMenu,
            selectReasoning: actions.selectReasoning,
            toggleApproval: actions.toggleApprovalMenu,
            selectApproval: actions.selectApproval,
            openGoal: actions.openGoalMenu,
            goalDraft: actions.goalDraft,
            saveGoal: actions.saveGoal,
            pauseGoal: actions.pauseGoal,
            resumeGoal: actions.resumeGoal,
            clearGoal: actions.clearGoal,
            openPlan: actions.openPlanMenu,
            selectPlanMode: actions.selectPlanMode,
            openMemory: actions.openMemoryMenu,
            selectMemory: actions.selectMemory,
            compactContext: actions.compactContext,
            moveSlashCommand: actions.moveSlashCommand,
            completeSlashCommand: actions.completeSlashCommand,
            addImages: actions.addImages,
            captureScreen: actions.captureScreen,
            removeImage: actions.removeImage,
            moveImage: actions.moveImage
          })}
        </div>
      </div>
    </section>
  `;
}

function operationIssue(model: ChatPanelModel): OperationIssueView | undefined {
  if (model.threadWorkspace.error) {
    return {
      source: 'thread',
      title: 'Thread 操作失败',
      detail: safeRuntimeIssueText(model.threadWorkspace.error, 220),
      retryable: false
    };
  }
  return undefined;
}

function skinMenu(model: ChatPanelModel, actions: ChatPanelActions) {
  return html`
    <details class="skin-menu">
      <summary class="icon-button" aria-label="Pet appearance">⋯</summary>
      <div class="skin-menu-popover">
        <label for="dock-skin-select">宠物外观</label>
        <select
          id="dock-skin-select"
          aria-label="Pet skin"
          @change=${(event: Event) => actions.selectSkin((event.currentTarget as HTMLSelectElement).value)}
        >
          ${model.skins.map((skin) => html`
            <option value=${skin.id} ?selected=${skin.selected}>${skin.displayName} · ${skin.origin}</option>
          `)}
        </select>
        <button class="action-button" type="button" @click=${actions.importSkin}>导入 .dockskin</button>
      </div>
    </details>
    <input
      data-testid="skin-file"
      type="file"
      accept=".dockskin,application/json"
      hidden
      @change=${(event: Event) => {
        const input = event.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        if (file) actions.skinFile(file);
        input.value = '';
      }}
    />
  `;
}

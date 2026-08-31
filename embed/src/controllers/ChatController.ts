import type { AgentSession } from '../session/AgentSession.js';
import type { SessionActivityState, SessionState } from '../session/SessionState.js';
import { PermissionController } from './PermissionController.js';
import type { PermissionDecision } from '../protocol/permissions.js';
import { cancellableTurn, TurnController } from './TurnController.js';
import type { SessionRuntimeIssue } from '../session/RuntimeIssueModel.js';
import { RuntimeIssueController } from './RuntimeIssueController.js';
import type { ApprovalMode, ReasoningEffort } from '../protocol/responses.js';
import {
  ThreadExecutionControlController,
  type ThreadMemorySelection
} from './ThreadExecutionControlController.js';
import { slashCommandSuggestions } from './SlashCommandModel.js';
import { browserDisplayCaptureSupported } from '../image-inputs/DisplayScreenshotProvider.js';
import type {
  ActiveTurnSendMode,
  ChatViewState,
  ComposerMenu,
  TurnIntentMode
} from './ChatViewState.js';
import { delay, resizeComposer } from './ChatDom.js';
import {
  ChatImageInputController,
  imageSubmissionError
} from './ChatImageInputController.js';
import { UserInputController } from './UserInputController.js';
import type { SessionUserInputRequest } from '../session/UserInputModel.js';
import { parseGoalCommand } from './GoalCommand.js';
import { ChatGoalController } from './ChatGoalController.js';

export type { ActiveTurnSendMode, ChatViewState } from './ChatViewState.js';

const ACTIVE_STATES = new Set<SessionActivityState>([
  'thinking',
  'streaming',
  'working',
  'approval',
  'input',
  'subagent'
]);

export class ChatController {
  private sessionValue?: AgentSession;
  private stateValue?: Readonly<SessionState>;
  private draftValue = '';
  private sendModeValue: ActiveTurnSendMode = 'queue';
  private turnIntentValue: TurnIntentMode = 'default';
  private composerMenu?: ComposerMenu;
  private modelChanging = false;
  private approvalChanging = false;
  private approvalConfirmationPending = false;
  private localError = '';
  private submitting = false;
  private capturingScreenshot = false;
  private slashCommandIndex = 0;
  private readonly imageInputs = new ChatImageInputController();
  private readonly removingQueueIds = new Set<string>();
  private removeSessionListener?: () => void;
  private lastScrolledRevision = '';
  private readonly permissionController: PermissionController;
  private readonly turnController: TurnController;
  private readonly runtimeIssueController: RuntimeIssueController;
  private readonly executionControl: ThreadExecutionControlController;
  private readonly userInputController: UserInputController;
  private readonly goalControl: ChatGoalController;

  constructor(private readonly onChange: () => void) {
    this.permissionController = new PermissionController(onChange);
    this.turnController = new TurnController(onChange);
    this.runtimeIssueController = new RuntimeIssueController(onChange, (value) => this.setDraft(value));
    this.executionControl = new ThreadExecutionControlController(onChange);
    this.userInputController = new UserInputController(onChange);
    this.goalControl = new ChatGoalController(onChange);
  }

  get session() {
    return this.sessionValue;
  }

  get state() {
    return this.stateValue;
  }

  get view(): ChatViewState {
    const activity = this.stateValue?.activity || 'idle';
    const busy = this.submitting || ACTIVE_STATES.has(activity);
    const sessionReady = this.stateValue?.connection === 'live';
    const activeTurn = Boolean(cancellableTurn(this.stateValue?.turns));
    const runtimeIssues = this.runtimeIssueController.visible(
      this.stateValue?.runtimeIssues || [],
      this.stateValue?.messages || []
    );
    const slashCommands = slashCommandSuggestions(this.draftValue, {
      activeTurn,
      imageSupported: this.stateValue?.environment?.model.inputModalities?.includes('image') === true,
      reusableImages: this.stateValue?.messages.some((message) =>
        message.role === 'user' && Boolean(message.attachments?.length)
      ) === true,
      screenCaptureSupported: browserDisplayCaptureSupported(),
      planSupported: this.stateValue?.environment?.plan.canChange === true,
      goalSupported: true,
      goalStatus: this.stateValue?.environment?.goal.status
    });
    const slashCommandIndex = Math.min(
      this.slashCommandIndex,
      Math.max(0, slashCommands.length - 1)
    );
    return {
      messages: this.stateValue?.messages || [],
      toolActivities: this.stateValue?.toolActivities || [],
      permissionRequests: this.stateValue?.permissionRequests || [],
      userInputRequests: this.stateValue?.userInputRequests || [],
      planActivities: this.stateValue?.planActivities || [],
      goalActivities: this.stateValue?.goalActivities || [],
      subagentActivities: this.stateValue?.subagentActivities || [],
      runtimeIssues,
      runtimeIssueInteractions: this.runtimeIssueController.interactions(runtimeIssues),
      turnQueue: this.stateValue?.turnQueue || [],
      permissionInteractions: this.permissionController.interactions,
      userInputInteractions: this.userInputController.interactions(
        this.stateValue?.userInputRequests || []
      ),
      draft: this.draftValue,
      pendingImages: this.imageInputs.pending,
      sendMode: this.sendModeValue,
      turnIntent: this.turnIntentValue,
      composerMenu: this.composerMenu,
      modelChanging: this.modelChanging,
      executionChanging: this.approvalChanging || this.executionControl.changing,
      approvalConfirmationPending: this.approvalConfirmationPending,
      goalDraft: this.goalControl.draft,
      goalChanging: this.goalControl.changing,
      environment: this.stateValue?.environment,
      activeTurn,
      activity,
      sessionReady,
      busy,
      submitting: this.submitting,
      capturingScreenshot: this.capturingScreenshot,
      canSend: sessionReady && !this.submitting && Boolean(this.draftValue.trim()),
      slashCommands,
      slashCommandIndex,
      turnControl: this.turnController.view,
      error: this.localError || this.goalControl.error || this.executionControl.error || undefined
    };
  }

  bind(session: AgentSession | undefined) {
    if (this.sessionValue === session) return;
    this.removeSessionListener?.();
    this.imageInputs.clear();
    this.sessionValue = session;
    this.composerMenu = undefined;
    this.goalControl.reset();
    this.permissionController.bind(session);
    this.turnController.bind(session);
    this.runtimeIssueController.bind(session);
    this.executionControl.bind(session);
    this.userInputController.bind(session);
    this.stateValue = session?.state;
    this.removeSessionListener = session?.onChange((state) => {
      this.stateValue = state;
      this.turnController.update(state);
      this.onChange();
    });
    this.turnController.update(this.stateValue);
    this.onChange();
  }

  setDraft(value: string) {
    if (this.draftValue === value) return;
    this.draftValue = value;
    this.slashCommandIndex = 0;
    this.localError = '';
    this.executionControl.clearError();
    this.onChange();
  }

  addImageFiles(files: readonly (File | Blob)[]) {
    try {
      this.imageInputs.add(
        files,
        this.stateValue?.environment?.model.inputModalities?.includes('image') === true
      );
      this.localError = '';
      this.onChange();
      return true;
    } catch (error) {
      this.localError = imageSubmissionError(error);
      this.onChange();
      return false;
    }
  }

  removeImage(id: string) {
    const changed = this.imageInputs.remove(id);
    if (changed) this.onChange();
    return changed;
  }

  moveImage(id: string, delta: number) {
    const changed = this.imageInputs.move(id, delta);
    if (changed) this.onChange();
    return changed;
  }

  moveSlashCommand(delta: number) {
    const commands = this.view.slashCommands;
    if (!commands.length) return;
    this.slashCommandIndex = (
      this.slashCommandIndex + delta + commands.length
    ) % commands.length;
    this.onChange();
  }

  completeSlashCommand(index = this.slashCommandIndex) {
    const command = this.view.slashCommands[index];
    if (!command?.available) return false;
    this.draftValue = `${command.name} `;
    this.slashCommandIndex = 0;
    this.localError = '';
    this.onChange();
    return true;
  }

  clearError() {
    if (!this.localError && !this.executionControl.error) return;
    this.localError = '';
    this.executionControl.clearError();
    this.onChange();
  }

  setSendMode(mode: ActiveTurnSendMode) {
    if (mode !== 'queue' && mode !== 'steer') return;
    if (this.sendModeValue === mode) return;
    this.sendModeValue = mode;
    this.localError = '';
    this.onChange();
  }

  toggleComposerMenu() {
    const opening = this.composerMenu !== 'context';
    this.composerMenu = opening ? 'context' : undefined;
    if (opening) void this.sessionValue?.refreshEnvironment().catch(() => undefined);
    this.onChange();
  }

  toggleModelMenu() {
    this.composerMenu = this.composerMenu === 'model' ? undefined : 'model';
    this.onChange();
  }

  openModelMenu() {
    if (this.composerMenu === 'model') return;
    this.composerMenu = 'model';
    this.onChange();
  }

  toggleReasoningMenu() {
    this.composerMenu = this.composerMenu === 'reasoning' ? undefined : 'reasoning';
    this.onChange();
  }

  openReasoningMenu() {
    if (this.composerMenu === 'reasoning') return;
    this.composerMenu = 'reasoning';
    this.onChange();
  }

  toggleApprovalMenu() {
    this.composerMenu = this.composerMenu === 'approval' ? undefined : 'approval';
    this.onChange();
  }

  openGoalMenu() {
    if (this.composerMenu === 'goal') return;
    this.composerMenu = 'goal';
    this.goalControl.reset();
    this.onChange();
  }

  openPlanMenu() {
    if (this.composerMenu === 'plan') return;
    this.composerMenu = 'plan';
    this.onChange();
  }

  openMemoryMenu() {
    if (this.composerMenu === 'memory') return;
    this.composerMenu = 'memory';
    this.onChange();
  }

  setGoalDraft(value: string) {
    this.goalControl.setDraft(value);
  }

  closeComposerMenu() {
    if (!this.composerMenu) return false;
    this.composerMenu = undefined;
    this.onChange();
    return true;
  }

  async selectModel(modelId: string) {
    const session = this.sessionValue;
    if (!session || this.modelChanging) return false;
    this.modelChanging = true;
    this.localError = '';
    this.onChange();
    try {
      await session.setModel(modelId);
      this.composerMenu = undefined;
      return true;
    } catch {
      this.localError = '模型未能切换，请刷新 Thread 状态后重试';
      return false;
    } finally {
      this.modelChanging = false;
      this.onChange();
    }
  }

  async refreshModels() {
    const session = this.sessionValue;
    if (!session || this.modelChanging) return false;
    this.modelChanging = true;
    this.localError = '';
    this.onChange();
    try {
      await session.refreshModels();
      return true;
    } catch {
      this.localError = '模型目录刷新失败，已保留上次可用目录';
      return false;
    } finally {
      this.modelChanging = false;
      this.onChange();
    }
  }

  selectReasoning(effort: ReasoningEffort) {
    return this.finishThreadControl(this.executionControl.selectReasoning(effort));
  }

  async selectApproval(mode: ApprovalMode) {
    const session = this.sessionValue;
    if (!session || this.approvalChanging || this.executionControl.changing) return false;
    this.approvalChanging = true;
    this.approvalConfirmationPending = false;
    this.localError = '';
    this.onChange();
    try {
      let result = await session.setApproval(mode);
      if ('confirmationRequired' in result) {
        this.approvalConfirmationPending = true;
        globalThis.open?.(result.confirmationUrl, '_blank', 'noopener,noreferrer');
        this.onChange();
        while ('confirmationRequired' in result && Date.now() < result.expiresAt) {
          await delay(600);
          if (this.sessionValue !== session) return false;
          result = await session.setApproval(mode, result.confirmationId);
        }
        if ('confirmationRequired' in result) throw new Error('confirmation expired');
      }
      this.composerMenu = undefined;
      return true;
    } catch {
      this.localError = mode === 'full_access'
        ? '完全访问尚未确认，请重新发起本机确认'
        : '审批模式未能切换，请刷新 Thread 状态后重试';
      return false;
    } finally {
      this.approvalChanging = false;
      this.approvalConfirmationPending = false;
      this.onChange();
    }
  }

  selectPlanMode(enabled: boolean) {
    this.turnIntentValue = enabled ? 'plan' : 'default';
    this.composerMenu = undefined;
    this.localError = '';
    this.onChange();
    return Promise.resolve(true);
  }

  selectMemory(selection: ThreadMemorySelection) {
    return this.finishThreadControl(this.executionControl.selectMemory(selection));
  }

  compactContext() {
    return this.executionControl.compactContext();
  }

  saveGoal() {
    return this.goalControl.save(this.sessionValue, this.stateValue);
  }

  pauseGoal() {
    return this.goalControl.pause(this.sessionValue, this.stateValue);
  }

  resumeGoal() {
    return this.goalControl.resume(this.sessionValue, this.stateValue);
  }

  clearGoal() {
    return this.goalControl.clear(this.sessionValue);
  }

  async submit() {
    const session = this.sessionValue;
    const originalDraft = this.draftValue;
    const message = originalDraft.trim();
    if (!message || this.submitting) return false;
    if (!session || session.state.connection !== 'live') {
      this.localError = 'Agent Session 尚未就绪';
      this.onChange();
      return false;
    }
    let goalCommand: ReturnType<typeof parseGoalCommand>;
    try {
      goalCommand = parseGoalCommand(message);
    } catch (error) {
      this.localError = error instanceof Error ? error.message : 'Goal 命令参数无效';
      this.onChange();
      return false;
    }
    if (goalCommand?.action === 'show') {
      this.draftValue = '';
      this.openGoalMenu();
      return true;
    }

    this.draftValue = '';
    this.localError = '';
    this.submitting = true;
    this.capturingScreenshot = /^\/screenshot(?:\s|$)/u.test(message);
    this.onChange();
    try {
      if (goalCommand) {
        await this.goalControl.execute(session, goalCommand);
        return true;
      }
      const planMatch = /^\/plan(?:\s+([\s\S]+))?$/u.exec(message);
      if ((planMatch || this.turnIntentValue === 'plan') && cancellableTurn(session.state.turns)) {
        throw new Error('Planning Turn 只能在空闲 Thread 中启动');
      }
      const planRequest = planMatch?.[1]?.trim() || '';
      if (planMatch && !planRequest) {
        throw new Error('/plan 后需要填写要规划的需求');
      }
      const prepared = this.imageInputs.submission(message);
      const base = typeof prepared === 'string' ? { message: prepared } : prepared;
      const submission = planMatch
        ? { ...base, message: planRequest, intent: { mode: 'plan' as const } }
        : this.turnIntentValue === 'plan'
          ? { ...base, intent: { mode: 'plan' as const } }
          : base;
      if (cancellableTurn(session.state.turns)) {
        if (this.sendModeValue === 'steer') await session.steerTurn(submission);
        else await session.enqueueTurn(submission);
      } else {
        await session.startTurn(submission);
      }
      this.turnIntentValue = 'default';
      this.imageInputs.clear();
      return true;
    } catch (error) {
      this.draftValue = originalDraft;
      this.localError = imageSubmissionError(error);
      return false;
    } finally {
      this.submitting = false;
      this.capturingScreenshot = false;
      this.onChange();
    }
  }

  resolvePermission(requestId: string, decision: PermissionDecision) {
    return this.permissionController.resolve(requestId, decision);
  }

  selectUserInput(interactionId: string, questionId: string, value: string) {
    this.userInputController.select(interactionId, questionId, value);
  }

  setUserInputOther(interactionId: string, questionId: string, value: string) {
    this.userInputController.setOther(interactionId, questionId, value);
  }

  submitUserInput(request: SessionUserInputRequest) {
    return this.userInputController.submit(request);
  }

  cancelTurn() {
    return this.turnController.cancelActiveTurn();
  }

  retryIssue(issue: SessionRuntimeIssue) {
    return this.runtimeIssueController.retry(issue);
  }

  editIssue(issue: SessionRuntimeIssue) {
    return this.runtimeIssueController.edit(issue);
  }

  dismissIssue(issueId: string) {
    this.runtimeIssueController.dismiss(issueId);
  }

  async removeQueuedTurn(queueId: string) {
    const session = this.sessionValue;
    if (!session || this.removingQueueIds.has(queueId)) return false;
    this.removingQueueIds.add(queueId);
    this.localError = '';
    this.onChange();
    try {
      const result = await session.removeQueuedTurn(queueId);
      if (!result.removed) throw new Error('排队消息已开始或不存在');
      return true;
    } catch {
      this.localError = '排队消息未能移除，请刷新状态后重试';
      return false;
    } finally {
      this.removingQueueIds.delete(queueId);
      this.onChange();
    }
  }

  afterRender(root: ParentNode) {
    resizeComposer(root.querySelector<HTMLTextAreaElement>('[data-testid="chat-input"]'));
    const list = root.querySelector<HTMLElement>('[data-testid="message-list"]');
    if (!list) {
      this.lastScrolledRevision = '';
      return;
    }
    const last = this.stateValue?.messages.at(-1);
    const revision = `${this.stateValue?.lastSeq || 0}:${last?.id || ''}:${last?.content.length || 0}`;
    if (revision === this.lastScrolledRevision) return;
    this.lastScrolledRevision = revision;
    list.scrollTop = list.scrollHeight;
  }

  destroy() {
    this.removeSessionListener?.();
    this.removeSessionListener = undefined;
    this.permissionController.destroy();
    this.runtimeIssueController.destroy();
    this.executionControl.bind(undefined);
    this.turnController.bind(undefined);
    this.sessionValue = undefined;
    this.stateValue = undefined;
    this.lastScrolledRevision = '';
    this.removingQueueIds.clear();
    this.composerMenu = undefined;
    this.goalControl.reset();
    this.imageInputs.clear();
  }

  private async finishThreadControl(operation: Promise<boolean>) {
    const changed = await operation;
    if (changed) this.composerMenu = undefined;
    this.onChange();
    return changed;
  }
}

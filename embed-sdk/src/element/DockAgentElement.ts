import { LitElement, html, nothing, type PropertyValues } from 'lit';
import { DockClient } from '../client/DockClient.js';
import type { ClientConnectionEvent } from '../client/ClientEvents.js';
import { chatPanel } from '../components/ChatPanel.js';
import { ChatController } from '../controllers/ChatController.js';
import { PetAnimator } from '../pet/PetAnimator.js';
import { PetLauncher } from '../pet/PetLauncher.js';
import { PetSkinLoader } from '../pet/PetSkinLoader.js';
import { PetSkinRegistry } from '../pet/PetSkinRegistry.js';
import { mapPetState, type PetStateSnapshot } from '../pet/PetStateMapper.js';
import { DUDU_SKIN } from '../pet/skins/default/atlas.js';
import { IndexedDbSkinStore } from '../pet/storage/IndexedDbSkinStore.js';
import type { LoadedPetSkin, PetSkinSummary } from '../pet/types.js';
import { elementStyles } from '../styles/elementStyles.js';
import { RecoveryController } from '../controllers/RecoveryController.js';
import { ThreadController } from '../controllers/ThreadController.js';
import { GatewayConnectionController } from '../controllers/GatewayConnectionController.js';
import { PairingController } from '../controllers/PairingController.js';
import { readAgentAttributes } from './attributes.js';
import {
  DOCK_AGENT_ERROR,
  DOCK_AGENT_READY,
  DOCK_AGENT_STATE,
  DOCK_AGENT_TOGGLE,
  type DockAgentPublicApi
} from './publicApi.js';

export class DockAgentElement extends LitElement implements DockAgentPublicApi {
  static properties = {
    application: { type: String, reflect: true },
    gatewayUrl: { type: String, attribute: 'gateway-url' },
    skin: { type: String, reflect: true },
    skinUrl: { type: String, attribute: 'skin-url', reflect: true },
    theme: { type: String, reflect: true }
  };

  static styles = elementStyles;

  application = '';
  gatewayUrl = '';
  skin = '';
  skinUrl = '';
  theme = 'auto';

  private clientValue?: DockClient;
  private gatewayConnection?: GatewayConnectionController;
  private pairingController?: PairingController;
  private connection: ClientConnectionEvent = { state: 'idle' };
  private readonly recoveryController = new RecoveryController(() => this.requestUpdate());
  private readonly chatController = new ChatController(() => {
    this.recoveryController.updateSession(this.chatController.state?.connection);
    this.refreshPetState();
    this.requestUpdate();
  });
  private readonly threadController = new ThreadController(
    () => {
      this.refreshPetState();
      this.requestUpdate();
    },
    (session) => this.chatController.bind(session)
  );
  private petState: PetStateSnapshot = mapPetState({ connection: 'idle' });
  private petStateSent = false;
  private readyDispatched = false;
  private successTimer?: ReturnType<typeof setTimeout>;
  private successSettled = false;
  private animator?: PetAnimator;
  private launcher?: PetLauncher;
  private skinLoader?: PetSkinLoader;
  private skinStore?: IndexedDbSkinStore;
  private selectedSkin?: LoadedPetSkin;
  private skinSummaries: PetSkinSummary[] = [];
  private warning = '';
  private openValue = false;
  private initialized = false;
  private initializing?: Promise<void>;
  private connecting?: Promise<void>;
  private removeConnectionListener?: () => void;

  get client() {
    return this.clientValue;
  }

  get open() {
    return this.openValue;
  }

  connectedCallback() {
    super.connectedCallback();
    if (this.hasUpdated) queueMicrotask(() => {
      if (this.isConnected) this.startUi();
    });
  }

  protected firstUpdated() {
    this.startUi();
  }

  private startUi() {
    if (this.launcher || !this.isConnected) return;
    const sprite = this.renderRoot.querySelector<HTMLElement>('[data-testid="pet-sprite"]');
    const shell = this.renderRoot.querySelector<HTMLElement>('[data-testid="pet-shell"]');
    const button = this.renderRoot.querySelector<HTMLElement>('[data-testid="pet-button"]');
    if (!sprite || !shell || !button) throw new Error('Dock pet shell failed to render');
    this.animator = new PetAnimator(sprite);
    this.launcher = new PetLauncher(shell, button, {
      onActivate: () => void this.toggleChat().catch((error) => this.fail(error)),
      onDragStart: (direction) => this.animator?.setState(`drag-${direction}`),
      onDragMove: (direction) => this.animator?.setState(`drag-${direction}`),
      onDragEnd: () => this.animator?.setState(this.petState.visual, true),
      onPosition: () => this.positionPanel()
    }, { application: this.application || 'unknown' });
    globalThis.addEventListener?.('keydown', this.keyDown);
    void this.initialize().catch((error) => this.fail(error));
  }

  protected updated(changed: PropertyValues<this>) {
    if (this.openValue) this.positionPanel();
    this.chatController.afterRender(this.renderRoot);
    if (this.initialized && (changed.has('skin') || changed.has('skinUrl'))) {
      void this.resolveInitialSkin().catch((error) => this.fail(error));
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    globalThis.removeEventListener?.('keydown', this.keyDown);
    this.removeConnectionListener?.();
    this.threadController.destroy();
    this.chatController.destroy();
    this.launcher?.destroy();
    this.animator?.destroy();
    this.clientValue?.disconnect();
    this.skinStore?.close();
    this.removeConnectionListener = undefined;
    this.launcher = undefined;
    this.animator = undefined;
    this.clientValue = undefined;
    this.gatewayConnection = undefined;
    this.pairingController = undefined;
    this.skinLoader = undefined;
    this.skinStore = undefined;
    this.initialized = false;
    this.initializing = undefined;
    this.connecting = undefined;
    this.connection = { state: 'idle' };
    this.readyDispatched = false;
    clearTimeout(this.successTimer);
    this.successTimer = undefined;
  }

  async connect() {
    await this.updateComplete;
    await this.initialize();
    this.connecting ||= this.connectOnce().finally(() => {
      this.connecting = undefined;
    });
    await this.connecting;
  }

  async openChat() {
    this.openValue = true;
    this.requestUpdate();
    await this.updateComplete;
    this.positionPanel();
    this.dispatchToggle();
    try {
      await this.connect();
    } catch {
      // connectOnce already dispatched dock:error
    }
    if (this.connection.state === 'connected') await this.ensureSession();
    await this.updateComplete;
    this.renderRoot.querySelector<HTMLTextAreaElement>('[data-testid="chat-input"]')?.focus();
  }

  closeChat() {
    if (!this.openValue) return;
    this.openValue = false;
    this.requestUpdate();
    this.dispatchToggle();
    queueMicrotask(() => this.renderRoot.querySelector<HTMLElement>('[data-testid="pet-button"]')?.focus());
  }

  async toggleChat() {
    if (this.openValue) this.closeChat();
    else await this.openChat();
  }

  async setSkin(id: string) {
    const loader = this.requireSkinLoader();
    const selected = await loader.select(this.application, id);
    this.warning = '';
    this.applySkin(selected);
    return selected;
  }

  async importSkin(file: File) {
    const selected = await this.requireSkinLoader().importFile(this.application, file);
    this.warning = '';
    this.applySkin(selected);
    return selected;
  }

  listSkins() {
    return [...this.skinSummaries];
  }

  render() {
    const status = this.petState.label;
    const chat = this.chatController.view;
    const threadWorkspace = this.threadController.view;
    const attentionLabel = threadWorkspace.attentionCount
      ? `，${threadWorkspace.attentionCount} 个后台会话等你处理`
      : '';
    return html`
      <div class="pet-shell" data-testid="pet-shell">
        <button
          class="pet-button"
          data-testid="pet-button"
          type="button"
          data-live=${String(this.petState.live)}
          aria-label=${`${status}${attentionLabel}。${this.openValue ? '关闭' : '打开'} Dock 对话`}
          aria-expanded=${String(this.openValue)}
          aria-controls="dock-chat-panel"
        >
          <div class="pet-sprite" data-testid="pet-sprite" role="img" aria-label=${status}></div>
          <span class="status-badge" aria-hidden="true">
            <span class="status-dot" data-live=${String(this.petState.live)}></span>
            <span class="status-label">${shortStatus(this.petState.visual)}</span>
          </span>
          ${threadWorkspace.attentionCount ? html`
            <span class="pet-attention-badge" aria-hidden="true">${threadWorkspace.attentionCount}</span>
          ` : nothing}
        </button>
      </div>
      ${chatPanel({
        ...chat,
        open: this.openValue,
        title: threadWorkspace.activeTitle,
        status,
        connected: this.connection.state === 'connected',
        recovery: this.recoveryController.view,
        pairing: this.pairingController?.view,
        gateway: this.gatewayConnection?.view,
        threadWorkspace,
        skins: this.skinSummaries,
        warning: this.warning
      }, {
        close: () => this.closeChat(),
        retry: () => void this.recoveryController.retry(() => this.connect()),
        refreshPairing: () => void this.pairingController?.begin(true),
        openGateway: () => this.gatewayConnection?.open(),
        gatewayDraft: (value) => this.gatewayConnection?.setDraft(value),
        connectGateway: () => void this.connectConfiguredGateway(),
        cancelGateway: () => this.gatewayConnection?.cancel(),
        draft: (value) => this.chatController.setDraft(value),
        sendMode: (mode) => this.chatController.setSendMode(mode),
        send: () => void this.chatController.submit(),
        toggleComposerMenu: () => this.chatController.toggleComposerMenu(),
        closeComposerMenu: () => this.chatController.closeComposerMenu(),
        toggleModelMenu: () => this.chatController.toggleModelMenu(),
        openModelMenu: () => this.chatController.openModelMenu(),
        selectModel: (modelId) => void this.chatController.selectModel(modelId),
        refreshModels: () => void this.chatController.refreshModels(),
        openReasoningMenu: () => this.chatController.openReasoningMenu(),
        selectReasoning: (effort) => void this.chatController.selectReasoning(effort),
        toggleApprovalMenu: () => this.chatController.toggleApprovalMenu(),
        selectApproval: (mode) => void this.chatController.selectApproval(mode),
        openGoalMenu: () => this.chatController.openGoalMenu(),
        goalDraft: (value) => this.chatController.setGoalDraft(value),
        saveGoal: () => void this.chatController.saveGoal(),
        pauseGoal: () => void this.chatController.pauseGoal(),
        resumeGoal: () => void this.chatController.resumeGoal(),
        clearGoal: () => void this.chatController.clearGoal(),
        openPlanMenu: () => this.chatController.openPlanMenu(),
        selectPlanMode: (enabled) => void this.chatController.selectPlanMode(enabled),
        openMemoryMenu: () => this.chatController.openMemoryMenu(),
        selectMemory: (selection) => void this.chatController.selectMemory(selection),
        compactContext: () => void this.chatController.compactContext(),
        moveSlashCommand: (delta) => this.chatController.moveSlashCommand(delta),
        completeSlashCommand: (index) => this.chatController.completeSlashCommand(index),
        addImages: (files) => this.chatController.addImageFiles(files),
        captureScreen: () => void this.chatController.captureScreen(),
        removeImage: (id) => this.chatController.removeImage(id),
        moveImage: (id, delta) => this.chatController.moveImage(id, delta),
        removeQueuedTurn: (queueId) => void this.chatController.removeQueuedTurn(queueId),
        selectSkin: (id) => void this.setSkin(id).catch((error) => this.fail(error)),
        importSkin: () => this.renderRoot.querySelector<HTMLInputElement>('[data-testid="skin-file"]')?.click(),
        skinFile: (file) => void this.importSkin(file).catch((error) => this.fail(error)),
        resolvePermission: (requestId, decision) => void this.chatController.resolvePermission(requestId, decision),
        resolvePlan: (planId, decision) => void this.chatController.resolvePlan(planId, decision),
        resolveElicitation: (elicitId, action) => void this.chatController.resolveElicitation(elicitId, action),
        selectUserInput: (interactionId, questionId, value) =>
          this.chatController.selectUserInput(interactionId, questionId, value),
        setUserInputOther: (interactionId, questionId, value) =>
          this.chatController.setUserInputOther(interactionId, questionId, value),
        submitUserInput: (request) => void this.chatController.submitUserInput(request),
        cancelTurn: () => void this.chatController.cancelTurn(),
        createThread: () => void this.threadController.create().catch(() => undefined),
        switchThread: (threadId) => void this.threadController.switchTo(threadId).catch(() => undefined),
        beginRenameThread: (threadId) => this.threadController.beginRename(threadId),
        renameDraft: (value) => this.threadController.setRenameDraft(value),
        saveRenameThread: (threadId) => void this.threadController.saveRename(threadId).catch(() => undefined),
        cancelRenameThread: () => this.threadController.cancelRename(),
        archiveThread: (threadId) => void this.threadController.archive(threadId).catch(() => undefined),
        restoreArchivedThread: (threadId) => void this.threadController.restoreArchived(threadId).catch(() => undefined),
        requestDeleteThread: (threadId) => this.threadController.requestDelete(threadId),
        confirmDeleteThread: (threadId) => void this.threadController.deletePermanently(threadId).catch(() => undefined),
        cancelDeleteThread: () => this.threadController.cancelDelete(),
        retryIssue: (issue) => void this.chatController.retryIssue(issue),
        editIssue: (issue) => void this.chatController.editIssue(issue),
        dismissIssue: (issueId) => this.chatController.dismissIssue(issueId),
        retryMessage: () => void this.chatController.submit(),
        clearMessageError: () => this.chatController.clearError(),
        clearThreadError: () => this.threadController.clearError(),
        dismissCommandOutput: () => this.chatController.dismissCommandOutput()
      })}
    `;
  }

  private initialize() {
    this.initializing ||= this.initializeOnce();
    return this.initializing;
  }

  private async initializeOnce() {
    if (this.initialized) return;
    const options = readAgentAttributes(this);
    this.application = options.application;
    this.gatewayConnection = new GatewayConnectionController({
      application: this.application,
      configuredGatewayUrl: options.gatewayUrl
    }, () => this.requestUpdate());
    this.gatewayUrl = this.gatewayConnection.view.gatewayUrl;
    this.theme = options.theme || 'auto';
    const registry = new PetSkinRegistry(DUDU_SKIN);
    this.skinStore = new IndexedDbSkinStore();
    this.skinLoader = new PetSkinLoader(registry, this.skinStore, {
      onWarning: (message) => {
        this.warning = message;
        this.requestUpdate();
      }
    });
    await this.resolveInitialSkin();
    this.installClient(this.gatewayUrl);
    this.initialized = true;
    if (options.autoConnect !== false) void this.connect().catch(() => undefined);
  }

  private installClient(gatewayUrl: string) {
    this.removeConnectionListener?.();
    this.removeConnectionListener = undefined;
    this.clientValue?.disconnect();
    this.chatController.bind(undefined);
    this.recoveryController.updateSession(undefined);
    this.clientValue = new DockClient({ application: this.application, gatewayUrl });
    this.pairingController = new PairingController({
      gatewayUrl: this.clientValue.gatewayUrl,
      origin: globalThis.location?.origin,
      request: () => this.requireClient().requestPairing(),
      waitForTicket: (id) => this.requireClient().waitForPairingTicket(id),
      onTicket: async (ticket) => {
        await this.requireClient().connectWithIssuedTicket(ticket);
      }
    }, () => {
      this.refreshPetState();
      this.requestUpdate();
    });
    this.gatewayUrl = this.clientValue.gatewayUrl;
    this.threadController.bind(this.clientValue);
    this.removeConnectionListener = this.clientValue.onConnectionChange((event) => {
      this.connection = event;
      // 掉线了：下次连上（含自动重连后的 connect）要重新报 ready。
      if (event.state !== 'connected') this.readyDispatched = false;
      this.recoveryController.updateConnection(event);
      if (event.state === 'connected') this.pairingController?.clear();
      else if (event.state === 'error' && needsPairing(event.code)) {
        void this.pairingController?.begin();
      }
      this.refreshPetState();
      this.requestUpdate();
    });
  }

  private async connectConfiguredGateway() {
    const gateway = this.gatewayConnection;
    const gatewayUrl = gateway?.beginConnect();
    if (!gateway || !gatewayUrl) return false;
    this.installClient(gatewayUrl);
    try {
      await this.connect();
      gateway.connected(gatewayUrl);
      return true;
    } catch {
      gateway.failed(this.clientValue?.connectionState.code);
      return false;
    }
  }

  private async resolveInitialSkin() {
    this.warning = '';
    const selected = await this.requireSkinLoader().resolve({
      application: this.application,
      requestedSkinId: this.skin || undefined,
      skinUrl: this.skinUrl || undefined
    });
    this.applySkin(selected);
  }

  private applySkin(skin: LoadedPetSkin) {
    this.selectedSkin = skin;
    this.animator?.setSkin(skin);
    this.skinSummaries = this.requireSkinLoader().registry.list(skin.manifest.id);
    this.requestUpdate();
  }

  private async connectOnce() {
    const client = this.requireClient();
    // 已经连上、也报过 ready（openChat 每次都会走一遍 connect）：不再重拉会话列表、不再重复发 dock:ready。
    if (client.connectionState.state === 'connected' && this.readyDispatched) return;
    try {
      if (client.connectionState.state !== 'connected') {
        try {
          await client.connect();
        } catch (error) {
          if (!needsPairing(errorCode(error)) || !this.pairingController) throw error;
          const started = await this.pairingController.begin();
          if (!started) {
            throw new Error(this.pairingController.view.error || '无法创建本机配对请求');
          }
          await this.pairingController.waitUntilBound();
        }
      }
      await this.afterConnected();
      this.readyDispatched = true;
      this.dispatchEvent(new CustomEvent(DOCK_AGENT_READY, {
        detail: { application: this.application, gatewayUrl: client.gatewayUrl }
      }));
    } catch (error) {
      this.dispatchError(error);
      throw error;
    }
  }

  private async afterConnected() {
    await this.threadController.load();
    await this.ensureSession();
  }

  private async ensureSession() {
    const session = await this.threadController.ensureActive();
    this.chatController.bind(session);
  }

  private refreshPetState() {
    const pairing = this.pairingController?.view;
    let next = mapPetState({
      connection: this.connection.state,
      session: this.chatController.state,
      pairing: Boolean(pairing?.visible && pairing.phase !== 'error')
    });
    // 「已完成」只亮一会儿：会话状态里 success 会一直留着（刷新页面后也是），宠物和会话头不能一直说 done。
    if (next.visual === 'success') {
      if (this.successSettled) next = { visual: 'idle', label: '已连接', live: next.live };
      else this.successTimer ??= setTimeout(() => {
        this.successTimer = undefined;
        this.successSettled = true;
        this.refreshPetState();
        this.requestUpdate();
      }, SUCCESS_SHOWN_MS);
    } else {
      clearTimeout(this.successTimer);
      this.successTimer = undefined;
      this.successSettled = false;
    }
    // 会话每个增量都会走到这里：状态真变了才通知宿主，不然 dock:state 一次连上就刷十几条一样的。
    if (samePetState(this.petState, next) && this.petStateSent) return;
    this.petState = next;
    this.petStateSent = true;
    this.animator?.setState(next.visual);
    this.dispatchEvent(new CustomEvent(DOCK_AGENT_STATE, { detail: { ...next } }));
  }

  private positionPanel() {
    if (!this.openValue || !this.launcher) return;
    const panel = this.renderRoot.querySelector<HTMLElement>('.chat-panel');
    if (!panel) return;
    const bounds = panel.getBoundingClientRect();
    const placement = this.launcher.panelPlacement({
      width: panel.offsetWidth || bounds.width || 380,
      height: panel.offsetHeight || bounds.height || 640
    });
    panel.dataset.side = placement.side;
    const left = `${placement.x}px`;
    const top = `${placement.y}px`;
    const tail = `${placement.anchorY}px`;
    if (
      panel.style.left === left
      && panel.style.top === top
      && panel.style.getPropertyValue('--pv-panel-tail-y') === tail
    ) {
      return;
    }
    panel.style.setProperty('--pv-panel-tail-y', tail);
    panel.style.left = left;
    panel.style.top = top;
  }

  private dispatchToggle() {
    this.dispatchEvent(new CustomEvent(DOCK_AGENT_TOGGLE, { detail: { open: this.openValue } }));
  }

  private readonly keyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
    if (event.key === 'Escape' && this.openValue) {
      event.preventDefault();
      if (this.chatController.closeComposerMenu()) return;
      this.closeChat();
    }
  };

  private fail(error: unknown) {
    this.warning = error instanceof Error ? error.message : String(error);
    this.dispatchError(error);
    this.requestUpdate();
  }

  private dispatchError(error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    this.dispatchEvent(new CustomEvent(DOCK_AGENT_ERROR, { detail: { message } }));
  }

  private requireClient() {
    if (!this.clientValue) throw new Error('Dock client is not initialized');
    return this.clientValue;
  }

  private requireSkinLoader() {
    if (!this.skinLoader) throw new Error('Dock skin loader is not initialized');
    return this.skinLoader;
  }
}

function shortStatus(state: string) {
  const labels: Record<string, string> = {
    sleeping: 'offline', connecting: 'connecting', idle: 'ready', talking: 'replying',
    thinking: 'thinking', working: 'working', approval: 'approval', subagent: 'subagent',
    success: 'done', error: 'error', 'drag-left': 'moving', 'drag-right': 'moving'
  };
  return labels[state] || state;
}

/** 一轮完成后「已完成」亮多久，再回到「已连接」。 */
const SUCCESS_SHOWN_MS = 4_000;

function samePetState(left: PetStateSnapshot, right: PetStateSnapshot) {
  return left.visual === right.visual && left.label === right.label && left.live === right.live;
}

function needsPairing(code: string | undefined) {
  return code === 'pairing_required' || code === 'binding_revoked' || code === 'origin_not_allowed';
}

function errorCode(error: unknown) {
  return error && typeof error === 'object' && 'code' in error
    ? String((error as { code: unknown }).code)
    : undefined;
}

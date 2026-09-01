import { HttpBootstrapClient } from '../bootstrap/HttpBootstrapClient.js';
import { PairingFlow } from '../bootstrap/PairingFlow.js';
import { TicketFlow } from '../bootstrap/TicketFlow.js';
import type { PairingRequestResult, TicketResult } from '../bootstrap/types.js';
import { DEFAULT_GATEWAY_URL } from '../bootstrap/GatewayUrlPolicy.js';
import { CONNECTION_AUTHENTICATE, INITIALIZE, WORKSPACE_LIST } from '../protocol/methods.js';
import type {
  AuthorizedWorkspace,
  ClientConnectionSnapshot,
  InitializeResult,
  WorkspaceListResult
} from '../protocol/responses.js';
import { DockClientError } from '../protocol/errors.js';
import type { AgentSessionOperations } from '../session/AgentSession.js';
import { assertProtocolVersion } from '../protocol/version.js';
import { JsonRpcPeer } from '../transport/JsonRpcPeer.js';
import {
  reconnectDelay,
  ReconnectPolicy
} from '../transport/ReconnectPolicy.js';
import {
  gatewayWebSocketUrl,
  WebSocketTransport,
  type WebSocketFactory
} from '../transport/WebSocketTransport.js';
import { DOCK_EMBED_VERSION } from '../version.js';
import { ThreadClient } from './ThreadClient.js';
import { defaultSessionStorage, ThreadStore, type StorageLike } from './ThreadStore.js';
import { TurnClient } from './TurnClient.js';
import { PermissionClient } from './PermissionClient.js';
import { InteractionClient } from './InteractionClient.js';
import { McpClient } from './McpClient.js';
import { SlashClient } from './SlashClient.js';
import { ContextClient } from './ContextClient.js';
import { ContextController } from '../controllers/ContextController.js';
import { ClientEventHub, type ClientConnectionListener } from './ClientEvents.js';
import { SessionCoordinator, type ThreadLifecycleListener } from './SessionCoordinator.js';
import type { HostContextItem } from '../protocol/context.js';
import type { HostContextClearOptions, HostContextProvider } from '../host/types.js';
import { HostToolsClient } from '../host-tools/HostToolsClient.js';
import { ImageInputsClient } from '../image-inputs/ImageInputsClient.js';
import type { ImageTurnInput, StartTurnInput } from '../image-inputs/types.js';
import type { CreateThreadOptions, DockClientOptions } from './ClientOptions.js';
import {
  assertSessionCapabilities,
  normalizeApplication,
  textValue
} from './ClientValidation.js';

export type { CreateThreadOptions, DockClientOptions } from './ClientOptions.js';

export class DockClient {
  readonly application: string;
  readonly gatewayUrl: string;
  readonly hostTools: HostToolsClient;
  readonly imageInputs: ImageInputsClient;
  readonly mcp: McpClient;
  private readonly pairing: PairingFlow;
  private readonly tickets: TicketFlow;
  private readonly timeoutMs: number;
  private readonly webSocketFactory?: WebSocketFactory;
  private readonly threadStore: ThreadStore;
  private readonly threadApi: ThreadClient;
  private readonly turnApi: TurnClient;
  private readonly permissionApi: PermissionClient;
  private readonly interactionApi: InteractionClient;
  private readonly slashApi: SlashClient;
  private readonly contextController: ContextController;
  private readonly sessionCoordinator: SessionCoordinator;
  private readonly reconnectPolicy?: ReconnectPolicy;
  private readonly clientInstanceId: string;
  private readonly events = new ClientEventHub();
  private workspaces: AuthorizedWorkspace[] = [];
  private origin = '';
  private peer?: JsonRpcPeer;
  private removeNotificationListener?: () => void;
  private removeCloseListener?: () => void;
  private reconnectGeneration = 0;
  private explicitlyDisconnected = false;

  constructor(options: DockClientOptions) {
    this.application = normalizeApplication(options.application);
    const clientStorage = options.storage === null
      ? undefined
      : options.storage || defaultSessionStorage();
    this.clientInstanceId = stableClientInstanceId(this.application, clientStorage);
    const http = new HttpBootstrapClient({
      gatewayUrl: options.gatewayUrl || DEFAULT_GATEWAY_URL,
      fetch: options.fetch
    });
    this.gatewayUrl = http.gatewayUrl;
    this.pairing = new PairingFlow(http, this.application);
    this.tickets = new TicketFlow(http, this.application);
    this.timeoutMs = options.requestTimeoutMs || 10_000;
    this.webSocketFactory = options.webSocketFactory;
    this.hostTools = new HostToolsClient();
    this.imageInputs = new ImageInputsClient();
    this.mcp = new McpClient((method, params) => this.request(method, params));
    this.threadStore = new ThreadStore(
      clientStorage,
      this.application
    );
    this.threadApi = new ThreadClient((method, params) => this.request(method, params));
    this.turnApi = new TurnClient((method, params) => this.request(method, params));
    this.permissionApi = new PermissionClient((method, params) => this.request(method, params));
    this.interactionApi = new InteractionClient((method, params) => this.request(method, params));
    this.slashApi = new SlashClient((method, params) => this.request(method, params));
    this.sessionCoordinator = new SessionCoordinator({
      threadApi: this.threadApi,
      threadStore: this.threadStore,
      createOperations: () => this.createSessionOperations(),
      requireWorkspace: (workspaceId) => this.requireWorkspace(workspaceId),
      origin: () => this.origin
    });
    this.contextController = new ContextController(
      new ContextClient((method, params) => this.request(method, params)),
      () => this.sessionCoordinator.activeSession ? {
        threadId: this.sessionCoordinator.activeSession.id,
        workspaceId: this.sessionCoordinator.activeSession.workspaceId
      } : undefined,
      () => Boolean(this.peer),
      {
        contextProvider: options.contextProvider,
        collectBrowserContext: options.collectBrowserContext
      }
    );
    this.reconnectPolicy = options.reconnect === false
      ? undefined
      : new ReconnectPolicy(options.reconnect);
  }

  get activeSession() {
    return this.sessionCoordinator.activeSession;
  }

  get activeWorkspaceId() {
    return this.sessionCoordinator.activeWorkspaceId;
  }

  get connectionState() {
    return this.events.current;
  }

  onConnectionChange(listener: ClientConnectionListener) {
    return this.events.onChange(listener);
  }

  requestPairing(): Promise<PairingRequestResult> {
    return this.pairing.begin();
  }

  pollPairing(pairingRequestId: string) {
    return this.pairing.poll(pairingRequestId);
  }

  waitForPairingTicket(pairingRequestId: string): Promise<TicketResult> {
    return this.pairing.waitUntilApproved(pairingRequestId);
  }

  async completePairing(pairingRequestId: string, _tokenValue?: string) {
    this.beginManualConnection();
    try {
      const exchanged = await this.pairing.complete(pairingRequestId);
      return await this.connectWithTicket(exchanged.ticket);
    } catch (error) {
      this.events.set('error', error);
      throw error;
    }
  }

  connectWithIssuedTicket(ticket: string) {
    this.beginManualConnection();
    return this.connectWithTicket(ticket).catch((error) => {
      this.events.set('error', error);
      throw error;
    });
  }

  async connect() {
    this.beginManualConnection();
    try {
      const issued = await this.tickets.acquire();
      return await this.connectWithTicket(issued.ticket);
    } catch (error) {
      this.events.set('error', error);
      throw error;
    }
  }

  disconnect() {
    this.explicitlyDisconnected = true;
    this.reconnectGeneration += 1;
    this.detachPeer(true);
    this.events.set('disconnected');
  }

  setContext(items: readonly HostContextItem[]) {
    return this.contextController.set(items);
  }

  clearContext(options: HostContextClearOptions) {
    return this.contextController.clear(options);
  }

  setContextProvider(provider?: HostContextProvider) {
    this.contextController.setProvider(provider);
  }

  listThreads(
    workspaceId = this.requireActiveWorkspace(),
    options: { includeArchived?: boolean } = {}
  ) {
    return this.sessionCoordinator.list(workspaceId, options);
  }

  async createThread(options: CreateThreadOptions = {}) {
    const workspaceId = options.workspaceId || this.requireActiveWorkspace();
    return this.sessionCoordinator.create(workspaceId, options.title);
  }

  restoreThread(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.sessionCoordinator.restore(threadId, workspaceId);
  }

  switchThread(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.restoreThread(threadId, workspaceId);
  }

  observeThread(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.sessionCoordinator.observe(threadId, workspaceId);
  }

  archiveThread(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.sessionCoordinator.archive(threadId, workspaceId);
  }

  renameThread(
    threadId: string,
    title: string,
    workspaceId = this.requireActiveWorkspace()
  ) {
    return this.sessionCoordinator.rename(threadId, title, workspaceId);
  }

  restoreArchivedThread(threadId: string, workspaceId = this.requireActiveWorkspace()) {
    return this.sessionCoordinator.restoreArchived(threadId, workspaceId);
  }

  deleteThread(
    threadId: string,
    workspaceId = this.requireActiveWorkspace(),
    confirmation = ''
  ) {
    return this.sessionCoordinator.delete(threadId, workspaceId, confirmation);
  }

  onThreadLifecycleChange(listener: ThreadLifecycleListener) {
    return this.sessionCoordinator.onLifecycleChange(listener);
  }

  switchWorkspace(workspaceIdValue: string) {
    return this.sessionCoordinator.switchWorkspace(textValue(workspaceIdValue));
  }

  getSession(threadId: string) {
    return this.sessionCoordinator.get(threadId);
  }

  startTurn(message: string | StartTurnInput) {
    const session = this.activeSession;
    if (!session) throw new DockClientError('thread_required', 'Select or create a Thread first');
    return session.startTurn(message);
  }

  enqueueTurn(message: string | StartTurnInput) {
    const session = this.activeSession;
    if (!session) throw new DockClientError('thread_required', 'Select or create a Thread first');
    return session.enqueueTurn(message);
  }

  steerTurn(message: string | StartTurnInput) {
    const session = this.activeSession;
    if (!session) throw new DockClientError('thread_required', 'Select or create a Thread first');
    return session.steerTurn(message);
  }

  private async connectWithTicket(ticketValue: string): Promise<ClientConnectionSnapshot> {
    this.detachPeer(true);
    let ticket = ticketValue;
    const transport = await WebSocketTransport.connect(
      gatewayWebSocketUrl(this.gatewayUrl),
      this.webSocketFactory
    );
    const peer = new JsonRpcPeer(transport, this.timeoutMs);
    try {
      await peer.request(CONNECTION_AUTHENTICATE, { ticket });
      ticket = '';
      const initialize = await peer.request<InitializeResult>(INITIALIZE, {
        clientInfo: {
          name: '@dock/embed',
          version: DOCK_EMBED_VERSION,
          instanceId: this.clientInstanceId
        }
      });
      assertProtocolVersion(initialize.protocolVersion);
      assertSessionCapabilities(initialize.capabilities);
      const workspaceResult = await peer.request<WorkspaceListResult>(WORKSPACE_LIST);
      await this.hostTools.attach(peer, {
        enabled: initialize.capabilities.hostTools === true,
        application: initialize.connection.application,
        origin: initialize.connection.origin,
        connectionLeaseId: initialize.connection.connectionLeaseId,
        workspaceIds: workspaceResult.workspaces.map((workspace) => workspace.id)
      });
      await this.imageInputs.attach(peer, {
        enabled: initialize.capabilities.imageInputs === true,
        application: initialize.connection.application,
        origin: initialize.connection.origin,
        connectionLeaseId: initialize.connection.connectionLeaseId,
        workspaceIds: workspaceResult.workspaces.map((workspace) => workspace.id)
      });
      this.peer = peer;
      this.origin = initialize.connection.origin;
      this.workspaces = workspaceResult.workspaces;
      this.attachPeer(peer);
      const snapshot = {
        initialize,
        workspaces: workspaceResult.workspaces,
        defaultWorkspaceId: workspaceResult.defaultWorkspaceId
      };
      await this.sessionCoordinator.recoverAfterConnect(
        workspaceResult.defaultWorkspaceId,
        workspaceResult.workspaces.map((workspace) => workspace.id)
      );
      this.events.set('connected');
      return snapshot;
    } catch (error) {
      this.hostTools.detach(peer);
      this.imageInputs.detach(peer);
      if (this.peer === peer) this.detachPeer(true);
      else peer.close();
      throw error;
    } finally {
      ticket = '';
    }
  }

  private createSessionOperations(): AgentSessionOperations {
    return {
      startTurn: async (
        id,
        selectedWorkspace,
        message,
        imageInputs?: readonly ImageTurnInput[],
        intent?: StartTurnInput['intent']
      ) => {
        const uploadedImages = imageInputs?.length
          ? await this.imageInputs.prepareTurn(id, selectedWorkspace, imageInputs)
          : undefined;
        const environment = await this.hostTools.prepareTurnEnvironment(
          () => this.contextController.prepareTurn(id, message)
        );
        const submission = await this.turnApi.start(
          id,
          selectedWorkspace,
          message,
          environment.context,
          environment.hostTools,
          uploadedImages,
          intent
        );
        const previews = uploadedImages?.flatMap((image) =>
          'preview' in image ? [image.preview] : []
        ) || [];
        if (previews.length && submission.turnId) {
          this.sessionCoordinator.get(id)?.setImagePreviews(submission.turnId, previews);
        }
        return submission;
      },
      enqueueTurn: async (id, workspace, message, imageInputs) => {
        const preparedImages = imageInputs?.length
          ? await this.imageInputs.prepareTurn(id, workspace, imageInputs)
          : undefined;
        const hostTools = await this.hostTools.prepareQueuedTurnEnvironment();
        return this.turnApi.enqueue(id, workspace, message, hostTools, preparedImages);
      },
      steerTurn: async (id, workspace, message, imageInputs) => {
        const preparedImages = imageInputs?.length
          ? await this.imageInputs.prepareTurn(id, workspace, imageInputs)
          : undefined;
        const hostTools = await this.hostTools.prepareQueuedTurnEnvironment();
        return this.turnApi.steer(id, workspace, message, hostTools, preparedImages);
      },
      listTurnQueue: (id, selectedWorkspace) => this.turnApi.listQueue(id, selectedWorkspace),
      removeQueuedTurn: (id, selectedWorkspace, queueId) => this.turnApi.removeQueued(
        id,
        selectedWorkspace,
        queueId
      ),
      cancelTurn: (id, selectedWorkspace, turnId) => this.turnApi.cancel(id, selectedWorkspace, turnId),
      refreshEnvironment: (id, selectedWorkspace) => this.threadApi.environment(id, selectedWorkspace),
      compactContext: (id, selectedWorkspace) => this.threadApi.compactContext(id, selectedWorkspace),
      setModel: (id, selectedWorkspace, modelId) => this.threadApi.setModel(
        id,
        selectedWorkspace,
        modelId
      ),
      refreshModels: (id, selectedWorkspace) => this.threadApi.refreshModels(
        id,
        selectedWorkspace
      ),
      setReasoning: (id, selectedWorkspace, effort) => this.threadApi.setReasoning(
        id,
        selectedWorkspace,
        effort
      ),
      setApproval: (id, selectedWorkspace, mode, confirmationId) => this.threadApi.setApproval(
        id,
        selectedWorkspace,
        mode,
        confirmationId
      ),
      setGoal: (id, selectedWorkspace, content) => this.threadApi.setGoal(
        id,
        selectedWorkspace,
        content
      ),
      editGoal: (id, selectedWorkspace, content, expectedRevision, operationId) =>
        this.threadApi.editGoal(
          id,
          selectedWorkspace,
          content,
          expectedRevision,
          operationId
        ),
      pauseGoal: (id, selectedWorkspace, expectedRevision, operationId) =>
        this.threadApi.pauseGoal(
          id,
          selectedWorkspace,
          expectedRevision,
          operationId
        ),
      completeGoal: (id, selectedWorkspace) => this.threadApi.completeGoal(id, selectedWorkspace),
      clearGoal: (id, selectedWorkspace, operationId) => this.threadApi.clearGoal(
        id,
        selectedWorkspace,
        operationId
      ),
      setPlanMode: (id, selectedWorkspace, enabled) => this.threadApi.setPlanMode(
        id,
        selectedWorkspace,
        enabled
      ),
      setMemory: (id, selectedWorkspace, selection) => this.threadApi.setMemory(
        id,
        selectedWorkspace,
        selection
      ),
      listSlashCommands: () => this.slashApi.list(),
      executeSlash: (text, threadId) => this.slashApi.execute(text, threadId),
      resolvePermission: (id, selectedWorkspace, requestId, turnId, decision) => this.permissionApi.resolve({
        requestId,
        threadId: id,
        turnId,
        workspaceId: selectedWorkspace,
        decision
      }),
      respondToInteraction: (
        id,
        selectedWorkspace,
        interactionId,
        turnId,
        answers
      ) => this.interactionApi.respond({
        interactionId,
        threadId: id,
        turnId,
        workspaceId: selectedWorkspace,
        answers
      })
    };
  }

  private attachPeer(peer: JsonRpcPeer) {
    this.removeNotificationListener = peer.onNotification((notification) => this.sessionCoordinator.dispatch(notification));
    this.removeCloseListener = peer.onClose(() => {
      if (this.peer !== peer) return;
      this.hostTools.detach(peer);
      this.imageInputs.detach(peer);
      this.removeNotificationListener?.();
      this.removeCloseListener?.();
      this.peer = undefined;
      this.sessionCoordinator.setDisconnected();
      this.events.set('disconnected');
      if (!this.explicitlyDisconnected) this.scheduleReconnect();
    });
  }

  private detachPeer(close: boolean) {
    const peer = this.peer;
    this.peer = undefined;
    this.hostTools.detach(peer);
    this.imageInputs.detach(peer);
    this.removeNotificationListener?.();
    this.removeCloseListener?.();
    this.removeNotificationListener = undefined;
    this.removeCloseListener = undefined;
    this.sessionCoordinator.setDisconnected();
    if (close) peer?.close();
  }

  private request<T>(method: string, params: Record<string, unknown> = {}) {
    if (!this.peer) {
      return Promise.reject<T>(new DockClientError('not_connected', 'Connect to Dock Gateway first'));
    }
    return this.peer.request<T>(method, params);
  }

  private requireWorkspace(workspaceId: string) {
    if (!this.workspaces.some((workspace) => workspace.id === workspaceId)) {
      throw new DockClientError('workspace_not_allowed', 'Workspace is not authorized for this connection');
    }
  }

  private requireActiveWorkspace() {
    if (!this.activeWorkspaceId) {
      throw new DockClientError('workspace_required', 'Select an authorized Workspace first');
    }
    return this.activeWorkspaceId;
  }

  private beginManualConnection() {
    this.explicitlyDisconnected = false;
    this.reconnectGeneration += 1;
    this.events.set('connecting');
  }

  private scheduleReconnect() {
    if (!this.reconnectPolicy) return;
    const generation = ++this.reconnectGeneration;
    this.events.set('reconnecting');
    void this.reconnect(generation);
  }

  private async reconnect(generation: number) {
    const policy = this.reconnectPolicy;
    if (!policy) return;
    let lastError: unknown;
    for (let attempt = 1; attempt <= policy.options.maxAttempts; attempt += 1) {
      await reconnectDelay(policy.delay(attempt));
      if (this.explicitlyDisconnected || generation !== this.reconnectGeneration) return;
      try {
        const issued = await this.tickets.acquire();
        await this.connectWithTicket(issued.ticket);
        return;
      } catch (error) {
        lastError = error;
        if (!policy.shouldRetry(error)) {
          if (!this.explicitlyDisconnected && generation === this.reconnectGeneration) {
            this.events.set('error', error);
          }
          return;
        }
      }
    }
    if (!this.explicitlyDisconnected && generation === this.reconnectGeneration) {
      this.events.set('error', lastError || 'Unable to reconnect to Dock Gateway');
    }
  }
}

function stableClientInstanceId(
  application: string,
  storage: Pick<StorageLike, 'getItem' | 'setItem'> | undefined
) {
  const key = `dock:${application}:client-instance`;
  let existing = '';
  try {
    existing = storage?.getItem(key) || '';
  } catch {
    // Storage denial makes this page instance intentionally non-resumable.
  }
  if (/^[A-Za-z0-9][A-Za-z0-9._:-]{7,127}$/.test(existing)) return existing;
  const generated = typeof globalThis.crypto?.randomUUID === 'function'
    ? globalThis.crypto.randomUUID()
    : `client-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
  try {
    storage?.setItem(key, generated);
  } catch {
    // The generated identity remains valid for this client lifetime only.
  }
  return generated;
}

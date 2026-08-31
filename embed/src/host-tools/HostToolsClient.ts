import { DockClientError } from '../protocol/errors.js';
import type { JsonRpcPeer } from '../transport/JsonRpcPeer.js';
import {
  digestHostToolCatalog,
  digestHostToolDescriptor,
  digestVisibleHostTools,
  HOST_TOOL_LIMITS,
  normalizeHostToolArguments,
  normalizeHostToolDescriptor,
  normalizeHostToolResult,
  normalizeHostToolScopes,
  randomHostToolId
} from './HostToolProtocol.js';
import type {
  HostToolDescriptor,
  HostToolHandler,
  HostToolRegistration,
  HostToolTurnEnvelope
} from './types.js';

type RegistrationRecord = {
  descriptor: HostToolDescriptor;
  descriptorDigest: Promise<string>;
  registrationEpoch: string;
  handler: HostToolHandler;
};

type ConnectionIdentity = {
  enabled: boolean;
  application: string;
  origin: string;
  connectionLeaseId: string;
  workspaceIds: string[];
};

export class HostToolsClient {
  readonly instanceId = randomHostToolId();
  private readonly registrations = new Map<string, RegistrationRecord>();
  private acknowledged = new Map<string, RegistrationRecord>();
  private completed = new Map<string, Promise<{ result: unknown }>>();
  private peer?: JsonRpcPeer;
  private identity?: ConnectionIdentity;
  private acknowledgedCatalogDigest = '';
  private acknowledgedCatalogGeneration = -1;
  private activeScopes: readonly string[] = Object.freeze([]);
  private environmentGeneration = 0;
  private catalogGeneration = 0;
  private removeInvokeHandler?: () => void;
  private removeNotificationListener?: () => void;
  private activeInvocations = new Map<string, AbortController>();
  private cancelledInvocations = new Set<string>();
  private syncQueue = Promise.resolve();

  get enabled() {
    return Boolean(this.identity?.enabled);
  }

  register(descriptorValue: HostToolDescriptor, handler: HostToolHandler): HostToolRegistration {
    if (typeof handler !== 'function') {
      throw new DockClientError('host_tool_handler_invalid', 'Host Tool handler must be a function');
    }
    if (this.registrations.size >= HOST_TOOL_LIMITS.toolsPerConnection
      && !this.registrations.has(descriptorValue.name)) {
      throw new DockClientError('host_tool_catalog_too_large', 'Host Tool catalog has too many tools');
    }
    const descriptor = normalizeHostToolDescriptor(descriptorValue);
    const registrationEpoch = randomHostToolId();
    const record: RegistrationRecord = {
      descriptor,
      descriptorDigest: digestHostToolDescriptor(descriptor),
      registrationEpoch,
      handler
    };
    this.registrations.set(descriptor.name, record);
    this.environmentGeneration += 1;
    this.catalogGeneration += 1;
    const synchronized = this.queueSynchronization();
    let disposed = false;
    return {
      name: descriptor.name,
      registrationEpoch,
      synchronized,
      dispose: async () => {
        if (disposed) return;
        disposed = true;
        if (this.registrations.get(descriptor.name) === record) {
          this.registrations.delete(descriptor.name);
          this.environmentGeneration += 1;
          this.catalogGeneration += 1;
          await this.queueSynchronization();
        }
      }
    };
  }

  setActiveScopes(value: readonly string[]) {
    const scopes = normalizeHostToolScopes(value);
    if (sameStrings(scopes, this.activeScopes)) return;
    this.activeScopes = scopes;
    this.environmentGeneration += 1;
  }

  async prepareTurnEnvironment<T>(collectContext: () => Promise<T>): Promise<{
    context: T;
    hostTools?: HostToolTurnEnvelope;
  }> {
    const generation = this.environmentGeneration;
    const context = await collectContext();
    await this.syncQueue;
    if (generation !== this.environmentGeneration) {
      throw new DockClientError(
        'host_environment_changed',
        'Host page scopes or tools changed while preparing the Turn snapshot'
      );
    }
    if (this.identity?.enabled && this.acknowledgedCatalogGeneration !== this.catalogGeneration) {
      throw new DockClientError('host_tools_unavailable', 'Host Tools catalog is not synchronized');
    }
    if (!this.identity?.enabled || !this.acknowledged.size) return { context };
    if (!this.acknowledgedCatalogDigest) {
      throw new DockClientError('host_tools_unavailable', 'Host Tools catalog is not synchronized');
    }
    return {
      context,
      hostTools: Object.freeze({
        toolCatalogDigest: this.acknowledgedCatalogDigest,
        activeScopes: Object.freeze([...this.activeScopes])
      })
    };
  }

  async prepareQueuedTurnEnvironment() {
    const environment = await this.prepareTurnEnvironment(async () => undefined);
    return environment.hostTools;
  }

  async attach(peer: JsonRpcPeer, identity: ConnectionIdentity) {
    this.detach();
    this.peer = peer;
    this.identity = identity;
    this.removeInvokeHandler = peer.handleRequest(
      'hostTools/invoke',
      (params) => this.invoke(params)
    );
    this.removeNotificationListener = peer.onNotification((notification) => {
      if (notification.method === 'hostTools/cancel') {
        this.cancelInvocation(notification.params);
      }
    });
    if (identity.enabled) {
      await this.synchronize(peer);
    }
  }

  detach(expectedPeer?: JsonRpcPeer) {
    if (expectedPeer && this.peer && expectedPeer !== this.peer) return;
    this.removeInvokeHandler?.();
    this.removeInvokeHandler = undefined;
    this.removeNotificationListener?.();
    this.removeNotificationListener = undefined;
    for (const controller of this.activeInvocations.values()) {
      controller.abort(new DockClientError(
        'host_tool_disconnected',
        'Host Tool connection was closed'
      ));
    }
    this.activeInvocations.clear();
    this.peer = undefined;
    this.identity = undefined;
    this.acknowledged = new Map();
    this.acknowledgedCatalogDigest = '';
    this.acknowledgedCatalogGeneration = -1;
  }

  private queueSynchronization() {
    const peer = this.peer;
    if (!peer || !this.identity?.enabled) return Promise.resolve();
    const operation = this.syncQueue.then(() => this.synchronize(peer));
    this.syncQueue = operation.catch(() => undefined);
    return operation;
  }

  private async synchronize(expectedPeer: JsonRpcPeer) {
    if (this.peer !== expectedPeer || !this.identity?.enabled) return;
    const catalogGeneration = this.catalogGeneration;
    const records = [...this.registrations.values()];
    const tools = await Promise.all(records.map(async (record) => ({
      ...record.descriptor,
      descriptorDigest: await record.descriptorDigest,
      registrationEpoch: record.registrationEpoch
    })));
    const toolCatalogDigest = await digestHostToolCatalog(tools);
    // The Gateway can invoke immediately after accepting hostTools/sync. Stage the
    // exact submitted catalog before sending so a response and callback delivered
    // in the same socket read cannot race the Promise continuation below. Keeping
    // the generation unacknowledged still prevents new Turns from using it.
    this.acknowledged = new Map(records.map((record) => [record.descriptor.name, record]));
    this.acknowledgedCatalogDigest = toolCatalogDigest;
    this.acknowledgedCatalogGeneration = -1;
    const result = await expectedPeer.request<{
      connectionLeaseId: string;
      toolCatalogDigest: string;
      acceptedToolNames: string[];
    }>('hostTools/sync', {
      instanceId: this.instanceId,
      toolCatalogDigest,
      tools
    });
    if (this.peer !== expectedPeer) {
      throw new DockClientError('host_tools_unavailable', 'Host Tools connection changed during synchronization');
    }
    if (
      result.connectionLeaseId !== this.identity.connectionLeaseId
      || result.toolCatalogDigest !== toolCatalogDigest
    ) {
      throw new DockClientError(
        'host_tool_catalog_ack_invalid',
        'Gateway returned an invalid Host Tools catalog acknowledgement'
      );
    }
    if (this.acknowledgedCatalogDigest === toolCatalogDigest) {
      this.acknowledgedCatalogGeneration = catalogGeneration;
    }
  }

  private invoke(value: Record<string, unknown>) {
    const identity = this.identity;
    if (!this.peer || !identity?.enabled) {
      throw new DockClientError('host_tools_unavailable', 'Host Tools connection is unavailable');
    }
    const invocationId = text(value.invocationId);
    if (!invocationId) {
      throw new DockClientError('host_tool_invocation_invalid', 'Host Tool invocationId is required');
    }
    const existing = this.completed.get(invocationId);
    if (existing) return existing;
    const execution = this.executeInvocation(value, identity);
    this.completed.set(invocationId, execution);
    while (this.completed.size > HOST_TOOL_LIMITS.completedInvocations) {
      const oldest = this.completed.keys().next().value;
      if (oldest) this.completed.delete(oldest);
      else break;
    }
    return execution;
  }

  private async executeInvocation(value: Record<string, unknown>, identity: ConnectionIdentity) {
    const name = text(value.name);
    const record = this.acknowledged.get(name);
    const descriptorDigest = record ? await record.descriptorDigest : '';
    const requestedScopes = normalizeHostToolScopes(value.activeScopes);
    const visible = await this.visibleTools(this.activeScopes);
    const visibleToolDigest = await digestVisibleHostTools(
      this.acknowledgedCatalogDigest,
      this.activeScopes,
      visible
    );
    const deadline = Number(value.deadline);
    if (
      !record
      || text(value.connectionLeaseId) !== identity.connectionLeaseId
      || text(value.application) !== identity.application
      || text(value.origin) !== identity.origin
      || !identity.workspaceIds.includes(text(value.workspaceId))
      || text(value.instanceId) !== this.instanceId
      || text(value.toolCatalogDigest) !== this.acknowledgedCatalogDigest
      || !sameStrings(requestedScopes, this.activeScopes)
      || text(value.visibleToolDigest) !== visibleToolDigest
      || !visible.some((tool) => tool.name === name)
      || text(value.descriptorDigest) !== descriptorDigest
      || text(value.registrationEpoch) !== record.registrationEpoch
    ) {
      throw new DockClientError(
        'host_tool_owner_mismatch',
        'Host Tool invocation does not belong to this registered Handler'
      );
    }
    if (!Number.isFinite(deadline) || deadline <= Date.now()) {
      throw new DockClientError('host_tool_timeout', 'Host Tool invocation deadline expired');
    }
    const argumentsValue = normalizeHostToolArguments(value.arguments, record.descriptor.inputSchema);
    const invocationId = text(value.invocationId);
    if (this.cancelledInvocations.has(invocationId)) {
      throw new DockClientError('host_tool_cancelled', 'Host Tool invocation was cancelled');
    }
    const controller = new AbortController();
    this.activeInvocations.set(invocationId, controller);
    const remaining = Math.min(
      Math.max(1, deadline - Date.now()),
      HOST_TOOL_LIMITS.handlerTimeoutMs
    );
    const timer = setTimeout(() => controller.abort(new DockClientError(
      'host_tool_timeout',
      'Host Tool handler timed out'
    )), remaining);
    try {
      const handler = Promise.resolve(record.handler(argumentsValue, {
        invocationId,
        workspaceId: text(value.workspaceId),
        signal: controller.signal
      }));
      const result = await Promise.race([
        handler,
        abortRejection(controller.signal)
      ]);
      return { result: normalizeHostToolResult(result) };
    } catch (error) {
      if (error instanceof DockClientError) throw error;
      throw new DockClientError('host_tool_handler_error', 'Host Tool handler failed');
    } finally {
      clearTimeout(timer);
      if (this.activeInvocations.get(invocationId) === controller) {
        this.activeInvocations.delete(invocationId);
      }
    }
  }

  private cancelInvocation(value: Record<string, unknown>) {
    const identity = this.identity;
    const invocationId = text(value.invocationId);
    if (
      !identity?.enabled
      || !invocationId
      || text(value.connectionLeaseId) !== identity.connectionLeaseId
      || text(value.instanceId) !== this.instanceId
    ) return;
    const controller = this.activeInvocations.get(invocationId);
    if (controller) {
      controller.abort(new DockClientError(
        'host_tool_cancelled',
        'Host Tool invocation was cancelled'
      ));
      return;
    }
    if (!this.completed.has(invocationId)) {
      this.cancelledInvocations.add(invocationId);
      while (this.cancelledInvocations.size > HOST_TOOL_LIMITS.completedInvocations) {
        const oldest = this.cancelledInvocations.values().next().value;
        if (oldest) this.cancelledInvocations.delete(oldest);
        else break;
      }
    }
  }

  private async visibleTools(activeScopes: readonly string[]) {
    const scopeSet = new Set(activeScopes);
    return Promise.all([...this.acknowledged.values()]
      .filter((record) => !record.descriptor.scopes?.length
        || record.descriptor.scopes.some((scope) => scopeSet.has(scope)))
      .map(async (record) => ({
        name: record.descriptor.name,
        descriptorDigest: await record.descriptorDigest,
        registrationEpoch: record.registrationEpoch
      })));
  }
}

function abortRejection(signal: AbortSignal) {
  return new Promise<never>((_resolve, reject) => {
    const aborted = () => reject(signal.reason instanceof Error
      ? signal.reason
      : new DockClientError('host_tool_cancelled', 'Host Tool invocation was cancelled'));
    if (signal.aborted) aborted();
    else signal.addEventListener('abort', aborted, { once: true });
  });
}

function text(value: unknown) {
  return String(value || '').trim();
}

function sameStrings(left: readonly string[], right: readonly string[]) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

import { ContextClient } from '../client/ContextClient.js';
import type { HostContextItem } from '../protocol/context.js';
import { ContextSnapshot } from '../host/ContextSnapshot.js';
import { HostBridge } from '../host/HostBridge.js';
import {
  normalizeContextItems,
  normalizeContextSource
} from '../host/ContextNormalizer.js';
import type {
  HostBridgeOptions,
  HostContextClearOptions,
  HostContextEventDetail,
  HostContextProvider
} from '../host/types.js';

interface ActiveContextTarget {
  threadId: string;
  workspaceId: string;
}

export class ContextController {
  private readonly bridge: HostBridge;

  constructor(
    private readonly client: ContextClient,
    private readonly activeTarget: () => ActiveContextTarget | undefined,
    private readonly connected: () => boolean,
    options: HostBridgeOptions = {}
  ) {
    this.bridge = new HostBridge(options, (detail) => this.applyHostEvent(detail));
  }

  setProvider(provider?: HostContextProvider) {
    this.bridge.setProvider(provider);
  }

  async set(items: readonly HostContextItem[]) {
    const grouped = this.bridge.set(items);
    const target = this.activeTarget();
    if (!target || !this.connected()) return;
    await Promise.all([...grouped].map(([source, sourceItems]) => this.client.replace(
      target.threadId,
      target.workspaceId,
      source,
      sourceItems
    )));
  }

  async clear(options: HostContextClearOptions) {
    const source = this.bridge.clear(options.source);
    const target = this.activeTarget();
    if (!target || !this.connected()) return;
    await this.client.remove(target.threadId, target.workspaceId, source);
  }

  async prepareTurn(threadId: string, userMessage: string) {
    const items = await this.bridge.itemsFor({
      threadId,
      userMessage,
      reason: 'turn_start'
    });
    return new ContextSnapshot(items, items.map((item) => item.source));
  }

  close() {
    this.bridge.close();
  }

  private async applyHostEvent(detail: HostContextEventDetail) {
    const source = normalizeContextSource(detail.source);
    if (detail.mode === 'remove') {
      await this.clear({ source });
      return;
    }
    if (detail.mode !== 'replace') throw new Error('Host Context event mode must be replace or remove');
    const items = normalizeContextItems(detail.items || []);
    if (!items.length) {
      await this.clear({ source });
      return;
    }
    if (items.some((item) => item.source !== source)) {
      throw new Error('Host Context event items must match the declared source');
    }
    await this.set(items);
  }
}

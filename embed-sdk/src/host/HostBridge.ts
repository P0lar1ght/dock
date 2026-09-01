import type { HostContextItem, NormalizedContextItem } from '../protocol/context.js';
import { normalizeContextItems, normalizeContextSource } from './ContextNormalizer.js';
import { HOST_CONTEXT_EVENT, hostContextEventDetail } from './HostEvents.js';
import type {
  HostBridgeOptions,
  HostBrowserEnvironment,
  HostContextEventDetail,
  HostContextProvider,
  HostContextRequest
} from './types.js';

type HostEventHandler = (detail: HostContextEventDetail) => void | Promise<void>;

export class HostBridge {
  private readonly pushed = new Map<string, readonly NormalizedContextItem[]>();
  private provider?: HostContextProvider;
  private readonly collectBrowser: boolean;
  private readonly browserEnvironment: () => HostBrowserEnvironment | undefined;
  private readonly eventHandler?: (event: Event) => void;

  constructor(options: HostBridgeOptions = {}, onHostEvent?: HostEventHandler) {
    this.provider = options.contextProvider;
    this.collectBrowser = options.collectBrowserContext !== false;
    this.browserEnvironment = options.browserEnvironment || currentBrowserEnvironment;
    if (onHostEvent && typeof window !== 'undefined') {
      this.eventHandler = (event) => {
        const detail = hostContextEventDetail(event);
        if (detail) void Promise.resolve(onHostEvent(detail)).catch(() => undefined);
      };
      window.addEventListener(HOST_CONTEXT_EVENT, this.eventHandler);
    }
  }

  setProvider(provider?: HostContextProvider) {
    this.provider = provider;
  }

  set(items: readonly HostContextItem[]) {
    const normalized = normalizeContextItems(items);
    const grouped = groupBySource(normalized);
    for (const [source, sourceItems] of grouped) this.pushed.set(source, sourceItems);
    return grouped;
  }

  clear(sourceValue: string) {
    const source = normalizeContextSource(sourceValue);
    this.pushed.delete(source);
    return source;
  }

  async itemsFor(request: HostContextRequest) {
    const combined: NormalizedContextItem[] = [];
    if (this.collectBrowser) {
      const browserItem = safeBrowserContext(this.browserEnvironment());
      if (browserItem) combined.push(...normalizeContextItems([browserItem]));
    }
    for (const items of this.pushed.values()) combined.push(...items);
    if (this.provider) {
      const provided = await this.provider(Object.freeze({ ...request }));
      combined.push(...normalizeContextItems(provided));
    }
    return deduplicate(combined);
  }

  close() {
    if (this.eventHandler && typeof window !== 'undefined') {
      window.removeEventListener(HOST_CONTEXT_EVENT, this.eventHandler);
    }
  }
}

function groupBySource(items: readonly NormalizedContextItem[]) {
  const grouped = new Map<string, NormalizedContextItem[]>();
  for (const item of items) {
    const sourceItems = grouped.get(item.source) || [];
    sourceItems.push(item);
    grouped.set(item.source, sourceItems);
  }
  return grouped;
}

function deduplicate(items: readonly NormalizedContextItem[]) {
  const values = new Map<string, NormalizedContextItem>();
  for (const item of items) values.set(`${item.source}\u0000${item.id}`, item);
  return [...values.values()];
}

function safeBrowserContext(environment: HostBrowserEnvironment | undefined): HostContextItem | undefined {
  if (!environment) return undefined;
  return {
    id: 'page',
    type: 'active_view',
    title: environment.title || 'Current page',
    summary: 'Safe browser metadata supplied by the embedded host SDK',
    source: 'sdk.browser',
    data: {
      origin: environment.origin,
      pathname: environment.pathname,
      language: environment.language,
      timeZone: environment.timeZone,
      visibility: environment.visibility
    },
    priority: 20,
    timestamp: Date.now()
  };
}

function currentBrowserEnvironment(): HostBrowserEnvironment | undefined {
  if (typeof document === 'undefined' || typeof location === 'undefined' || typeof navigator === 'undefined') {
    return undefined;
  }
  return {
    title: document.title,
    origin: location.origin,
    pathname: location.pathname,
    language: navigator.language,
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || '',
    visibility: document.visibilityState
  };
}

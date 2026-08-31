import {
  DEFAULT_GATEWAY_URL,
  normalizeGatewayUrl
} from '../bootstrap/GatewayUrlPolicy.js';
import { GatewayPreferenceStore } from '../client/GatewayPreferenceStore.js';
import type { StorageLike } from '../client/ThreadStore.js';

export type GatewayUrlSource = 'configured' | 'saved' | 'default';

export interface GatewayConnectionView {
  gatewayUrl: string;
  source: GatewayUrlSource;
  canChange: boolean;
  editing: boolean;
  draft: string;
  connecting: boolean;
  error?: string;
}

export interface GatewayConnectionControllerOptions {
  application: string;
  configuredGatewayUrl?: string;
  storage?: StorageLike;
}

export class GatewayConnectionController {
  private readonly configuredGatewayUrl: string;
  private readonly store: GatewayPreferenceStore;
  private gatewayUrlValue: string;
  private sourceValue: GatewayUrlSource;
  private editing = false;
  private draftValue = '';
  private connecting = false;
  private error = '';

  constructor(
    options: GatewayConnectionControllerOptions,
    private readonly onChange: () => void
  ) {
    this.configuredGatewayUrl = options.configuredGatewayUrl
      ? normalizeGatewayUrl(options.configuredGatewayUrl)
      : '';
    this.store = new GatewayPreferenceStore(options.application, options.storage);
    const saved = this.configuredGatewayUrl ? '' : this.store.read();
    this.gatewayUrlValue = this.configuredGatewayUrl || saved || DEFAULT_GATEWAY_URL;
    this.sourceValue = this.configuredGatewayUrl ? 'configured' : saved ? 'saved' : 'default';
  }

  get view(): GatewayConnectionView {
    return {
      gatewayUrl: this.gatewayUrlValue,
      source: this.sourceValue,
      canChange: !this.configuredGatewayUrl,
      editing: this.editing,
      draft: this.draftValue,
      connecting: this.connecting,
      error: this.error || undefined
    };
  }

  open() {
    if (this.configuredGatewayUrl || this.connecting) return false;
    this.editing = true;
    this.draftValue = this.gatewayUrlValue;
    this.error = '';
    this.onChange();
    return true;
  }

  setDraft(value: string) {
    if (!this.editing || this.connecting) return;
    this.draftValue = value.slice(0, 200);
    this.error = '';
    this.onChange();
  }

  cancel() {
    if (!this.editing || this.connecting) return;
    this.editing = false;
    this.draftValue = '';
    this.error = '';
    this.onChange();
  }

  beginConnect() {
    if (!this.editing || this.connecting || this.configuredGatewayUrl) return undefined;
    try {
      const gatewayUrl = normalizeGatewayUrl(this.draftValue);
      this.gatewayUrlValue = gatewayUrl;
      this.draftValue = gatewayUrl;
      this.connecting = true;
      this.error = '';
      this.onChange();
      return gatewayUrl;
    } catch {
      this.error = '请输入本机 Gateway 地址，例如 http://127.0.0.1:18990';
      this.onChange();
      return undefined;
    }
  }

  connected(gatewayUrl: string) {
    this.gatewayUrlValue = this.store.write(gatewayUrl);
    this.sourceValue = 'saved';
    this.editing = false;
    this.draftValue = '';
    this.connecting = false;
    this.error = '';
    this.onChange();
  }

  failed(code?: string) {
    if (code && code !== 'gateway_unreachable') {
      this.gatewayUrlValue = this.store.write(this.gatewayUrlValue);
      this.sourceValue = 'saved';
      this.editing = false;
      this.draftValue = '';
      this.error = '';
    } else {
      this.editing = true;
      this.error = '无法连接这个地址，请确认本机 Gateway 端口后重试';
    }
    this.connecting = false;
    this.onChange();
  }
}

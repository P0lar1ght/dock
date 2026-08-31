import { normalizeGatewayUrl } from '../bootstrap/GatewayUrlPolicy.js';
import type { StorageLike } from './ThreadStore.js';

export class GatewayPreferenceStore {
  constructor(
    private readonly application: string,
    private readonly storage: StorageLike | undefined = defaultLocalStorage()
  ) {}

  read() {
    const key = this.key();
    try {
      const value = this.storage?.getItem(key);
      if (!value) return '';
      try {
        return normalizeGatewayUrl(value);
      } catch {
        this.storage?.removeItem(key);
        return '';
      }
    } catch {
      return '';
    }
  }

  write(gatewayUrl: string) {
    const normalized = normalizeGatewayUrl(gatewayUrl);
    try {
      this.storage?.setItem(this.key(), normalized);
    } catch {
      // A browser may deny storage; the live connection can still continue.
    }
    return normalized;
  }

  clear() {
    try {
      this.storage?.removeItem(this.key());
    } catch {
      // Preference storage is optional and never affects authorization.
    }
  }

  private key() {
    return `dock:${encodeURIComponent(this.application)}:gateway-url`;
  }
}

export function defaultLocalStorage(): StorageLike | undefined {
  try {
    return typeof globalThis.localStorage === 'undefined' ? undefined : globalThis.localStorage;
  } catch {
    return undefined;
  }
}

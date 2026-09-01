import { defaultSessionStorage, type StorageLike } from './ThreadStore.js';

export class ThreadAttentionStore {
  constructor(
    private readonly application: string,
    private readonly storage: StorageLike | undefined = defaultSessionStorage()
  ) {}

  seenSeq(threadId: string) {
    try {
      const raw = this.storage?.getItem(this.key(threadId));
      if (raw === null || raw === undefined || raw === '') return undefined;
      const value = Number(raw);
      return Number.isSafeInteger(value) && value >= 0 ? value : undefined;
    } catch {
      return undefined;
    }
  }

  markSeen(threadId: string, seq: number) {
    try {
      this.storage?.setItem(this.key(threadId), String(Math.max(0, Math.floor(seq))));
    } catch {
      // Attention cursors are per-tab hints; Gateway history remains authoritative.
    }
  }

  clear(threadId: string) {
    try {
      this.storage?.removeItem(this.key(threadId));
    } catch {
      // Storage may be disabled by the embedding browser.
    }
  }

  private key(threadId: string) {
    return `dock:${encodeURIComponent(this.application)}:thread:${encodeURIComponent(threadId)}:seen-seq`;
  }
}

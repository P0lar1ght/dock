export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export class ThreadStore {
  constructor(
    private readonly storage: StorageLike | undefined,
    private readonly application: string
  ) {}

  activeWorkspace(origin: string) {
    return this.read(this.key(origin, 'active-workspace'));
  }

  setActiveWorkspace(origin: string, workspaceId: string) {
    this.write(this.key(origin, 'active-workspace'), workspaceId);
  }

  lastThread(origin: string, workspaceId: string) {
    return this.read(this.key(origin, `workspace:${workspaceId}:last-thread`));
  }

  setLastThread(origin: string, workspaceId: string, threadId: string) {
    this.write(this.key(origin, `workspace:${workspaceId}:last-thread`), threadId);
  }

  clearLastThread(origin: string, workspaceId: string) {
    try {
      this.storage?.removeItem(this.key(origin, `workspace:${workspaceId}:last-thread`));
    } catch {
      // Storage is only a convenience; Gateway history remains authoritative.
    }
  }

  private key(origin: string, suffix: string) {
    return `dock:${encodeURIComponent(this.application)}:${encodeURIComponent(origin)}:${suffix}`;
  }

  private read(key: string) {
    try {
      return String(this.storage?.getItem(key) || '').trim();
    } catch {
      return '';
    }
  }

  private write(key: string, value: string) {
    try {
      this.storage?.setItem(key, value);
    } catch {
      // Private browsing and embedded policies may disable storage.
    }
  }
}

export function defaultSessionStorage(): StorageLike | undefined {
  try {
    return typeof globalThis.sessionStorage === 'undefined' ? undefined : globalThis.sessionStorage;
  } catch {
    return undefined;
  }
}

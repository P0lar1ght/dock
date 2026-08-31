import type { LoadedPetSkin } from '../types.js';

const DATABASE_NAME = 'dock-embed-pets';
const DATABASE_VERSION = 1;
const SKINS = 'skins';
const PREFERENCES = 'preferences';

interface SkinRecord {
  id: string;
  skin: LoadedPetSkin;
}

interface PreferenceRecord {
  key: string;
  skinId: string;
}

export class IndexedDbSkinStore {
  private database?: Promise<IDBDatabase>;
  private readonly memorySkins = new Map<string, LoadedPetSkin>();
  private readonly memoryPreferences = new Map<string, string>();

  constructor(private readonly indexedDb: IDBFactory | undefined = globalThis.indexedDB) {}

  async list() {
    if (!this.indexedDb) return [...this.memorySkins.values()];
    const records = await this.request<SkinRecord[]>(SKINS, 'readonly', (store) => store.getAll());
    return records.map((record) => record.skin);
  }

  async get(id: string) {
    if (!this.indexedDb) return this.memorySkins.get(id);
    const record = await this.request<SkinRecord | undefined>(SKINS, 'readonly', (store) => store.get(id));
    return record?.skin;
  }

  async put(skin: LoadedPetSkin) {
    if (skin.origin !== 'local') throw new Error('Only browser-local skins can be persisted');
    if (!this.indexedDb) {
      this.memorySkins.set(skin.manifest.id, skin);
      return;
    }
    await this.request<IDBValidKey>(SKINS, 'readwrite', (store) => store.put({ id: skin.manifest.id, skin }));
  }

  async remove(id: string) {
    if (!this.indexedDb) {
      this.memorySkins.delete(id);
      return;
    }
    await this.request<undefined>(SKINS, 'readwrite', (store) => store.delete(id));
  }

  async selectedSkin(application: string) {
    const key = preferenceKey(application);
    if (!this.indexedDb) return this.memoryPreferences.get(key);
    const record = await this.request<PreferenceRecord | undefined>(PREFERENCES, 'readonly', (store) => store.get(key));
    return record?.skinId;
  }

  async setSelectedSkin(application: string, skinId?: string) {
    const key = preferenceKey(application);
    if (!this.indexedDb) {
      if (skinId) this.memoryPreferences.set(key, skinId);
      else this.memoryPreferences.delete(key);
      return;
    }
    await this.request(PREFERENCES, 'readwrite', (store) => skinId
      ? store.put({ key, skinId })
      : store.delete(key));
  }

  close() {
    void this.database?.then((database) => database.close());
    this.database = undefined;
  }

  private request<T>(
    storeName: string,
    mode: IDBTransactionMode,
    operation: (store: IDBObjectStore) => IDBRequest
  ): Promise<T> {
    return this.open().then((database) => new Promise<T>((resolve, reject) => {
      const transaction = database.transaction(storeName, mode);
      const request = operation(transaction.objectStore(storeName));
      request.onsuccess = () => resolve(request.result as T);
      request.onerror = () => reject(request.error || new Error('IndexedDB request failed'));
      transaction.onabort = () => reject(transaction.error || new Error('IndexedDB transaction aborted'));
    }));
  }

  private open() {
    if (!this.indexedDb) throw new Error('IndexedDB is unavailable');
    this.database ||= new Promise<IDBDatabase>((resolve, reject) => {
      const request = this.indexedDb!.open(DATABASE_NAME, DATABASE_VERSION);
      request.onupgradeneeded = () => {
        const database = request.result;
        if (!database.objectStoreNames.contains(SKINS)) database.createObjectStore(SKINS, { keyPath: 'id' });
        if (!database.objectStoreNames.contains(PREFERENCES)) database.createObjectStore(PREFERENCES, { keyPath: 'key' });
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error || new Error('Unable to open pet skin storage'));
    });
    return this.database;
  }
}

function preferenceKey(application: string) {
  const origin = typeof location === 'undefined' ? 'unknown-origin' : location.origin;
  return `${application}\u0000${origin}`;
}

import { IndexedDbSkinStore } from './storage/IndexedDbSkinStore.js';
import { PetSkinRegistry } from './PetSkinRegistry.js';
import {
  PET_SKIN_LIMITS,
  parsePetSkinArchive,
  resolvePetAssetUrl,
  validateAtlasDataUrl,
  validatePetAtlasImage,
  validatePetSkinArchive,
  validatePetSkinManifest
} from './PetSkinValidator.js';
import {
  PET_SKIN_FORMAT,
  type LoadedPetSkin,
  type PetSkinArchive,
  type PetSkinLoadOptions,
  type PetSkinManifest,
  type PetSkinSelection
} from './types.js';

export class PetSkinLoader {
  private readonly fetcher: typeof globalThis.fetch;
  private readonly maxArchiveBytes: number;
  private readonly maxAtlasBytes: number;
  private readonly validateImage: (url: string, manifest: PetSkinManifest) => Promise<void>;
  private readonly warning: (message: string) => void;
  private initialized = false;

  constructor(
    readonly registry: PetSkinRegistry,
    readonly store: IndexedDbSkinStore,
    options: PetSkinLoadOptions = {}
  ) {
    this.fetcher = options.fetch || globalThis.fetch.bind(globalThis);
    this.maxArchiveBytes = options.maxArchiveBytes || PET_SKIN_LIMITS.maxArchiveBytes;
    this.maxAtlasBytes = options.maxAtlasBytes || PET_SKIN_LIMITS.maxAtlasBytes;
    this.validateImage = options.validateImage || validatePetAtlasImage;
    this.warning = options.onWarning || (() => undefined);
  }

  async initialize() {
    if (this.initialized) return;
    this.initialized = true;
    for (const stored of await this.store.list()) {
      try {
        const manifest = validatePetSkinManifest(stored.manifest);
        validateAtlasDataUrl(stored.atlasUrl, manifest.sprite.mimeType);
        this.registry.register({ ...stored, manifest, origin: 'local' });
      } catch {
        await this.store.remove(stored.manifest?.id || '');
      }
    }
  }

  async resolve(selection: PetSkinSelection) {
    await this.initialize();
    const preferredId = await this.store.selectedSkin(selection.application);
    if (preferredId) {
      const preferred = this.registry.get(preferredId);
      if (preferred) return preferred;
      await this.store.setSelectedSkin(selection.application);
    }
    if (selection.requestedSkinId) {
      const requested = this.registry.get(selection.requestedSkinId);
      if (requested) return requested;
      this.warning(`Unknown pet skin "${selection.requestedSkinId}"; using the next available source`);
    }
    if (selection.skinUrl) {
      try {
        return await this.loadUrl(selection.skinUrl);
      } catch (error) {
        this.warning(error instanceof Error ? error.message : 'Unable to load URL pet skin');
      }
    }
    return this.registry.fallback();
  }

  async select(application: string, id: string) {
    await this.initialize();
    const skin = this.registry.require(id);
    await this.store.setSelectedSkin(application, id);
    return skin;
  }

  async clearSelection(application: string) {
    await this.store.setSelectedSkin(application);
  }

  async importFile(application: string, file: Pick<File, 'name' | 'size' | 'text'>) {
    if (!file.name.toLowerCase().endsWith('.dockskin')) throw new Error('Local pet skins must use the .dockskin extension');
    if (file.size > this.maxArchiveBytes) throw new Error('Pet skin archive exceeds the size limit');
    const archive = parsePetSkinArchive(await file.text(), this.maxArchiveBytes);
    if (!archive.atlasDataUrl) throw new Error('Local .dockskin archives must contain an inline atlas image');
    const skin = await this.fromArchive(archive, 'local');
    this.registry.register(skin);
    await this.store.put(skin);
    await this.store.setSelectedSkin(application, skin.manifest.id);
    return skin;
  }

  async removeLocal(application: string, id: string) {
    const skin = this.registry.get(id);
    if (!skin || skin.origin !== 'local') return false;
    this.registry.remove(id);
    await this.store.remove(id);
    if (await this.store.selectedSkin(application) === id) await this.store.setSelectedSkin(application);
    return true;
  }

  private async loadUrl(urlValue: string) {
    const pageUrl = typeof location === 'undefined' ? undefined : location.href;
    const manifestUrl = resolvePetAssetUrl(urlValue, pageUrl);
    const response = await this.fetcher(manifestUrl, {
      credentials: 'omit',
      mode: 'cors',
      redirect: 'error',
      referrerPolicy: 'no-referrer'
    });
    if (!response.ok) throw new Error(`Pet skin manifest request failed (${response.status})`);
    const text = await boundedText(response, this.maxArchiveBytes);
    const parsed = JSON.parse(text) as unknown;
    let archive: PetSkinArchive;
    if (isArchive(parsed)) {
      archive = validatePetSkinArchive(parsed);
    } else {
      archive = {
        format: PET_SKIN_FORMAT,
        formatVersion: 1,
        manifest: validatePetSkinManifest(parsed)
      };
    }
    const skin = await this.fromArchive(archive, 'url', manifestUrl);
    this.registry.register(skin);
    return skin;
  }

  private async fromArchive(archive: PetSkinArchive, origin: 'local' | 'url', baseUrl?: string): Promise<LoadedPetSkin> {
    const manifest = validatePetSkinManifest(archive.manifest);
    let atlasUrl = archive.atlasDataUrl;
    if (!atlasUrl) {
      const source = manifest.sprite.src;
      if (!source) throw new Error('Pet skin does not declare an atlas source');
      atlasUrl = await this.fetchAtlas(resolvePetAssetUrl(source, baseUrl), manifest.sprite.mimeType);
    }
    validateAtlasDataUrl(atlasUrl, manifest.sprite.mimeType);
    await this.validateImage(atlasUrl, manifest);
    return { manifest, atlasUrl, origin, loadedAt: Date.now() };
  }

  private async fetchAtlas(url: string, mimeType: string) {
    if (url.startsWith('data:')) return url;
    const response = await this.fetcher(url, {
      credentials: 'omit',
      mode: 'cors',
      redirect: 'error',
      referrerPolicy: 'no-referrer'
    });
    if (!response.ok) throw new Error(`Pet atlas request failed (${response.status})`);
    const length = Number(response.headers.get('content-length'));
    if (length > this.maxAtlasBytes) throw new Error('Pet atlas exceeds the size limit');
    const blob = await response.blob();
    if (blob.size > this.maxAtlasBytes) throw new Error('Pet atlas exceeds the size limit');
    if (blob.type && blob.type !== mimeType) throw new Error('Pet atlas MIME type does not match the manifest');
    return blobDataUrl(blob, mimeType);
  }
}

async function boundedText(response: Response, maxBytes: number) {
  const length = Number(response.headers.get('content-length'));
  if (length > maxBytes) throw new Error('Pet skin manifest exceeds the size limit');
  const text = await response.text();
  if (new TextEncoder().encode(text).byteLength > maxBytes) throw new Error('Pet skin manifest exceeds the size limit');
  return text;
}

function blobDataUrl(blob: Blob, mimeType: string) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error || new Error('Unable to read pet atlas'));
    reader.onload = () => {
      const value = String(reader.result || '');
      resolve(value.replace(/^data:[^;,]+/, `data:${mimeType}`));
    };
    reader.readAsDataURL(blob);
  });
}

function isArchive(value: unknown): value is PetSkinArchive {
  return Boolean(value && typeof value === 'object' && 'manifest' in value);
}

import type { LoadedPetSkin, PetSkinOrigin, PetSkinSummary } from './types.js';

export class PetSkinRegistry {
  private readonly skins = new Map<string, LoadedPetSkin>();

  constructor(private readonly defaultSkin: LoadedPetSkin) {
    if (defaultSkin.origin !== 'builtin') throw new Error('Default pet skin must be built in');
    this.register(defaultSkin);
  }

  get defaultId() {
    return this.defaultSkin.manifest.id;
  }

  register(skin: LoadedPetSkin) {
    const existing = this.skins.get(skin.manifest.id);
    if (existing?.origin === 'builtin' && skin.origin !== 'builtin') {
      throw new Error(`Cannot replace built-in pet skin: ${skin.manifest.id}`);
    }
    this.skins.set(skin.manifest.id, skin);
    return skin;
  }

  remove(id: string) {
    const skin = this.skins.get(id);
    if (!skin || skin.origin === 'builtin') return false;
    return this.skins.delete(id);
  }

  get(id: string | undefined) {
    return id ? this.skins.get(id) : undefined;
  }

  require(id: string) {
    const skin = this.get(id);
    if (!skin) throw new Error(`Unknown pet skin: ${id}`);
    return skin;
  }

  fallback() {
    return this.defaultSkin;
  }

  list(selectedId?: string): PetSkinSummary[] {
    return [...this.skins.values()]
      .sort((left, right) => originOrder(left.origin) - originOrder(right.origin)
        || left.manifest.displayName.localeCompare(right.manifest.displayName))
      .map((skin) => ({
        id: skin.manifest.id,
        displayName: skin.manifest.displayName,
        origin: skin.origin,
        selected: skin.manifest.id === selectedId
      }));
  }
}

function originOrder(origin: PetSkinOrigin) {
  return origin === 'builtin' ? 0 : origin === 'local' ? 1 : 2;
}

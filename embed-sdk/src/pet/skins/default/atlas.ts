import atlasUrl from './spritesheet.webp?inline';
import type { LoadedPetSkin } from '../../types.js';
import { DUDU_MANIFEST } from './manifest.js';

export const DUDU_SKIN: LoadedPetSkin = Object.freeze({
  manifest: DUDU_MANIFEST,
  atlasUrl,
  origin: 'builtin',
  loadedAt: 0
});

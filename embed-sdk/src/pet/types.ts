export const PET_SKIN_FORMAT = 'dock.pet-skin' as const;
export const PET_SKIN_VERSION = 1 as const;
export const PET_SPRITE_VERSION = 2 as const;

export const PET_ANIMATION_NAMES = [
  'idle',
  'talking',
  'thinking',
  'working',
  'approval',
  'success',
  'error',
  'sleeping',
  'connecting',
  'subagent',
  'drag-left',
  'drag-right'
] as const;

export type PetAnimationName = typeof PET_ANIMATION_NAMES[number];
export type PetVisualState = PetAnimationName;
export type PetSkinOrigin = 'builtin' | 'local' | 'url';

export interface PetAtlasDefinition {
  columns: number;
  rows: number;
  cellWidth: number;
  cellHeight: number;
  width: number;
  height: number;
}

export interface PetAnimationDefinition {
  row: number;
  frames: number;
  fps: number;
  loop: boolean;
}

export interface PetSpriteDefinition {
  mimeType: 'image/png' | 'image/webp';
  src?: string;
}

export interface PetSkinManifest {
  format: typeof PET_SKIN_FORMAT;
  formatVersion: typeof PET_SKIN_VERSION;
  id: string;
  displayName: string;
  description?: string;
  spriteVersionNumber: typeof PET_SPRITE_VERSION;
  atlas: PetAtlasDefinition;
  sprite: PetSpriteDefinition;
  animations: Record<PetAnimationName, PetAnimationDefinition>;
}

/**
 * Portable .dockskin files are JSON-only archives. They cannot contain HTML,
 * JavaScript, event handlers, or executable URLs.
 */
export interface PetSkinArchive {
  format: typeof PET_SKIN_FORMAT;
  formatVersion: typeof PET_SKIN_VERSION;
  manifest: PetSkinManifest;
  atlasDataUrl?: string;
}

export interface LoadedPetSkin {
  manifest: PetSkinManifest;
  atlasUrl: string;
  origin: PetSkinOrigin;
  loadedAt: number;
}

export interface PetSkinSummary {
  id: string;
  displayName: string;
  origin: PetSkinOrigin;
  selected: boolean;
}

export interface PetSkinSelection {
  application: string;
  requestedSkinId?: string;
  skinUrl?: string;
}

export interface PetPosition {
  x: number;
  y: number;
}

export interface NormalizedPetPosition {
  x: number;
  y: number;
}

export interface PetSize {
  width: number;
  height: number;
}

export interface PetViewport {
  width: number;
  height: number;
}

export interface PetLauncherCallbacks {
  onActivate: () => void;
  onDragStart?: (direction: 'left' | 'right') => void;
  onDragMove?: (direction: 'left' | 'right') => void;
  onDragEnd?: () => void;
  onPosition?: (position: PetPosition) => void;
}

export interface PetAnimatorOptions {
  reducedMotion?: boolean;
  requestFrame?: (callback: FrameRequestCallback) => number;
  cancelFrame?: (handle: number) => void;
  now?: () => number;
  onSettled?: (state: PetVisualState) => void;
}

export interface PetSkinLoadOptions {
  fetch?: typeof globalThis.fetch;
  maxArchiveBytes?: number;
  maxAtlasBytes?: number;
  validateImage?: (url: string, manifest: PetSkinManifest) => Promise<void>;
  onWarning?: (message: string) => void;
}

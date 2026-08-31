import {
  PET_SKIN_FORMAT,
  PET_SKIN_VERSION,
  PET_SPRITE_VERSION,
  type PetSkinManifest
} from '../../types.js';

export const DUDU_MANIFEST = Object.freeze({
  format: PET_SKIN_FORMAT,
  formatVersion: PET_SKIN_VERSION,
  id: 'dudu',
  displayName: '嘟嘟',
  description: 'A faithful, rounded orange 3D Dudu companion with its green strap and doll charm.',
  spriteVersionNumber: PET_SPRITE_VERSION,
  atlas: {
    columns: 8,
    rows: 11,
    cellWidth: 192,
    cellHeight: 208,
    width: 1536,
    height: 2288
  },
  sprite: {
    mimeType: 'image/webp'
  },
  animations: {
    idle: { row: 0, frames: 6, fps: 6, loop: true },
    'drag-right': { row: 1, frames: 8, fps: 12, loop: true },
    'drag-left': { row: 2, frames: 8, fps: 12, loop: true },
    talking: { row: 3, frames: 4, fps: 8, loop: true },
    success: { row: 4, frames: 5, fps: 10, loop: false },
    error: { row: 5, frames: 8, fps: 6, loop: true },
    approval: { row: 6, frames: 6, fps: 5, loop: true },
    connecting: { row: 6, frames: 6, fps: 7, loop: true },
    working: { row: 7, frames: 6, fps: 9, loop: true },
    subagent: { row: 7, frames: 6, fps: 12, loop: true },
    thinking: { row: 8, frames: 6, fps: 5, loop: true },
    sleeping: { row: 0, frames: 6, fps: 2, loop: true }
  }
} satisfies PetSkinManifest);

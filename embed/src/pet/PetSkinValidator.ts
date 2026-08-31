import {
  PET_ANIMATION_NAMES,
  PET_SKIN_FORMAT,
  PET_SKIN_VERSION,
  PET_SPRITE_VERSION,
  type PetAnimationDefinition,
  type PetAnimationName,
  type PetSkinArchive,
  type PetSkinManifest
} from './types.js';

export const PET_SKIN_LIMITS = Object.freeze({
  maxArchiveBytes: 6 * 1024 * 1024,
  maxAtlasBytes: 5 * 1024 * 1024,
  maxAtlasWidth: 2048,
  maxAtlasHeight: 3072,
  maxFrames: 8,
  maxFps: 24,
  maxTextLength: 256,
  maxJsonDepth: 8
});

const ID_PATTERN = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const DATA_URL_PATTERN = /^data:image\/(?:png|webp);base64,[a-z0-9+/=\s]+$/i;
const DANGEROUS_KEY = /^(?:__proto__|prototype|constructor|on[a-z]+)$/i;

export class PetSkinValidationError extends Error {
  readonly code = 'invalid_pet_skin';
}

export function parsePetSkinArchive(input: string, maxBytes = PET_SKIN_LIMITS.maxArchiveBytes) {
  if (new TextEncoder().encode(input).byteLength > maxBytes) fail('Skin archive exceeds the size limit');
  let parsed: unknown;
  try {
    parsed = JSON.parse(input);
  } catch {
    fail('Skin archive must be valid JSON');
  }
  assertStaticJson(parsed, 0);
  return validatePetSkinArchive(parsed);
}

export function validatePetSkinArchive(input: unknown): PetSkinArchive {
  const archive = record(input, 'skin archive');
  if (archive.format !== PET_SKIN_FORMAT || archive.formatVersion !== PET_SKIN_VERSION) {
    fail('Unsupported skin archive format');
  }
  const manifest = validatePetSkinManifest(archive.manifest);
  const atlasDataUrl = optionalString(
    archive.atlasDataUrl,
    'atlasDataUrl',
    Math.ceil(PET_SKIN_LIMITS.maxAtlasBytes * 4 / 3) + 128
  );
  if (atlasDataUrl) validateAtlasDataUrl(atlasDataUrl, manifest.sprite.mimeType);
  if (!atlasDataUrl && !manifest.sprite.src) fail('Skin archive must declare an atlas asset');
  return { format: PET_SKIN_FORMAT, formatVersion: PET_SKIN_VERSION, manifest, atlasDataUrl: atlasDataUrl || undefined };
}

export function validatePetSkinManifest(input: unknown): PetSkinManifest {
  assertStaticJson(input, 0);
  const manifest = record(input, 'skin manifest');
  if (manifest.format !== PET_SKIN_FORMAT || manifest.formatVersion !== PET_SKIN_VERSION) {
    fail('Unsupported skin manifest format');
  }
  if (manifest.spriteVersionNumber !== PET_SPRITE_VERSION) fail('Skin must use sprite version 2');
  const id = requiredString(manifest.id, 'id', 64).toLowerCase();
  if (!ID_PATTERN.test(id)) fail('Skin id must be a stable lowercase identifier');
  const displayName = requiredString(manifest.displayName, 'displayName');
  const description = optionalString(manifest.description, 'description');
  const atlas = validateAtlas(manifest.atlas);
  const sprite = validateSprite(manifest.sprite);
  const sourceAnimations = record(manifest.animations, 'animations');
  const animations = {} as Record<PetAnimationName, PetAnimationDefinition>;
  for (const name of PET_ANIMATION_NAMES) {
    animations[name] = validateAnimation(sourceAnimations[name], name, atlas.rows);
  }
  return {
    format: PET_SKIN_FORMAT,
    formatVersion: PET_SKIN_VERSION,
    id,
    displayName,
    description: description || undefined,
    spriteVersionNumber: PET_SPRITE_VERSION,
    atlas,
    sprite,
    animations
  };
}

export function resolvePetAssetUrl(value: string, baseUrl?: string) {
  if (DATA_URL_PATTERN.test(value)) return value;
  let resolved: URL;
  try {
    resolved = baseUrl ? new URL(value, baseUrl) : new URL(value);
  } catch {
    fail('Skin asset URL is invalid');
  }
  if (resolved.protocol !== 'http:' && resolved.protocol !== 'https:') {
    fail('Skin assets must use HTTP, HTTPS, or an image data URL');
  }
  if (resolved.username || resolved.password) fail('Skin asset URLs cannot contain credentials');
  return resolved.href;
}

export function validateAtlasDataUrl(value: string, mimeType: string) {
  if (!DATA_URL_PATTERN.test(value)) fail('Atlas data must be a base64 PNG or WebP image');
  if (!value.startsWith(`data:${mimeType};base64,`)) fail('Atlas MIME type does not match the manifest');
  const encoded = value.slice(value.indexOf(',') + 1).replace(/\s/g, '');
  const estimatedBytes = Math.floor(encoded.length * 0.75);
  if (estimatedBytes > PET_SKIN_LIMITS.maxAtlasBytes) fail('Atlas image exceeds the size limit');
}

export async function validatePetAtlasImage(url: string, manifest: PetSkinManifest) {
  if (typeof Image === 'undefined') return;
  const dimensions = await new Promise<{ width: number; height: number }>((resolve, reject) => {
    const image = new Image();
    image.decoding = 'async';
    image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
    image.onerror = () => reject(new PetSkinValidationError('Atlas image could not be decoded'));
    image.src = url;
  });
  if (dimensions.width !== manifest.atlas.width || dimensions.height !== manifest.atlas.height) {
    fail('Atlas dimensions do not match the manifest');
  }
}

function validateAtlas(input: unknown) {
  const atlas = record(input, 'atlas');
  const columns = integer(atlas.columns, 'atlas.columns', 1, 8);
  const rows = integer(atlas.rows, 'atlas.rows', 1, 11);
  const cellWidth = integer(atlas.cellWidth, 'atlas.cellWidth', 16, 256);
  const cellHeight = integer(atlas.cellHeight, 'atlas.cellHeight', 16, 256);
  const width = integer(atlas.width, 'atlas.width', 16, PET_SKIN_LIMITS.maxAtlasWidth);
  const height = integer(atlas.height, 'atlas.height', 16, PET_SKIN_LIMITS.maxAtlasHeight);
  if (columns !== 8 || rows !== 11 || cellWidth !== 192 || cellHeight !== 208) {
    fail('V1 embedded skins must use the 8x11 Dock atlas contract');
  }
  if (width !== columns * cellWidth || height !== rows * cellHeight) fail('Atlas geometry is inconsistent');
  return { columns, rows, cellWidth, cellHeight, width, height };
}

function validateSprite(input: unknown) {
  const sprite = record(input, 'sprite');
  const mimeType = requiredString(sprite.mimeType, 'sprite.mimeType', 32);
  if (mimeType !== 'image/png' && mimeType !== 'image/webp') fail('Atlas must be PNG or WebP');
  const src = optionalString(sprite.src, 'sprite.src', 2048);
  if (src && /^(?:javascript|file|blob):/i.test(src)) fail('Executable or local asset URLs are not allowed');
  return { mimeType, src: src || undefined } as const;
}

function validateAnimation(input: unknown, name: string, rowCount: number): PetAnimationDefinition {
  const animation = record(input, `animation ${name}`);
  return {
    row: integer(animation.row, `${name}.row`, 0, rowCount - 1),
    frames: integer(animation.frames, `${name}.frames`, 1, PET_SKIN_LIMITS.maxFrames),
    fps: integer(animation.fps, `${name}.fps`, 1, PET_SKIN_LIMITS.maxFps),
    loop: Boolean(animation.loop)
  };
}

function assertStaticJson(value: unknown, depth: number) {
  if (depth > PET_SKIN_LIMITS.maxJsonDepth) fail('Skin JSON exceeds the depth limit');
  if (Array.isArray(value)) return value.forEach((item) => assertStaticJson(item, depth + 1));
  if (!value || typeof value !== 'object') return;
  for (const [key, item] of Object.entries(value)) {
    if (DANGEROUS_KEY.test(key)) fail('Skin JSON contains an executable or unsafe field');
    assertStaticJson(item, depth + 1);
  }
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(`${name} must be an object`);
  return value as Record<string, unknown>;
}

function requiredString(value: unknown, name: string, max: number = PET_SKIN_LIMITS.maxTextLength) {
  const text = typeof value === 'string' ? value.trim() : '';
  if (!text) fail(`${name} is required`);
  if (text.length > max) fail(`${name} exceeds the length limit`);
  return text;
}

function optionalString(value: unknown, name: string, max: number = PET_SKIN_LIMITS.maxTextLength) {
  if (value === undefined || value === null) return '';
  if (typeof value !== 'string') fail(`${name} must be a string`);
  if (value.length > max) fail(`${name} exceeds the length limit`);
  return value.trim();
}

function integer(value: unknown, name: string, min: number, max: number) {
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < min || number > max) fail(`${name} is out of range`);
  return number;
}

function fail(message: string): never {
  throw new PetSkinValidationError(message);
}

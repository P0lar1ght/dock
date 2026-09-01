import { DockClientError } from '../protocol/errors.js';
import type { ManualImageTurnInput } from './types.js';

const MAX_IMAGES = 4;
const MAX_SOURCE_BYTES = 16 * 1024 * 1024;
const ALLOWED_MIME = new Set(['image/png', 'image/jpeg', 'image/webp']);

export interface PendingImageInput {
  id: string;
  name: string;
  previewUrl: string;
  mimeType: 'image/png' | 'image/jpeg' | 'image/webp';
  byteLength: number;
}

interface StoredPendingImage extends PendingImageInput {
  blob: Blob;
}

/** Page-lifetime owner of manual image Blobs and their local preview URLs. */
export class PendingImageInputStore {
  private images: StoredPendingImage[] = [];

  add(values: readonly (File | Blob)[]) {
    if (!values.length) return this.snapshot();
    if (this.images.length + values.length > MAX_IMAGES) {
      throw new DockClientError(
        'image_input_invalid',
        `A Turn accepts at most ${MAX_IMAGES} images`
      );
    }
    const candidates = values.map((blob, index) => validate(blob, this.images.length + index));
    for (const candidate of candidates) {
      this.images.push({
        ...candidate,
        id: randomId(),
        previewUrl: globalThis.URL.createObjectURL(candidate.blob)
      });
    }
    return this.snapshot();
  }

  remove(idValue: string) {
    const id = String(idValue || '').trim();
    const index = this.images.findIndex((image) => image.id === id);
    if (index < 0) return false;
    const [removed] = this.images.splice(index, 1);
    if (removed) globalThis.URL.revokeObjectURL(removed.previewUrl);
    return true;
  }

  move(idValue: string, deltaValue: number) {
    const id = String(idValue || '').trim();
    const index = this.images.findIndex((image) => image.id === id);
    const target = index + Math.sign(Number(deltaValue) || 0);
    if (index < 0 || target < 0 || target >= this.images.length) return false;
    const [image] = this.images.splice(index, 1);
    if (!image) return false;
    this.images.splice(target, 0, image);
    return true;
  }

  inputs(): readonly ManualImageTurnInput[] {
    return this.images.map((image) => Object.freeze({
      type: 'image',
      blob: image.blob,
      name: image.name,
      detail: 'auto'
    }));
  }

  snapshot(): readonly PendingImageInput[] {
    return this.images.map(({ blob: _blob, ...image }) => Object.freeze({ ...image }));
  }

  clear() {
    for (const image of this.images) globalThis.URL.revokeObjectURL(image.previewUrl);
    this.images = [];
  }
}

function validate(blob: File | Blob, index: number) {
  if (!(blob instanceof Blob) || !ALLOWED_MIME.has(blob.type)) {
    throw new DockClientError(
      'image_input_mime_unsupported',
      'Only PNG, JPEG, and static WebP images are supported'
    );
  }
  if (blob.size < 1 || blob.size > MAX_SOURCE_BYTES) {
    throw new DockClientError(
      'image_input_too_large',
      'Each source image must be between 1 byte and 16 MiB'
    );
  }
  const fileName = typeof File !== 'undefined' && blob instanceof File ? blob.name : '';
  return {
    blob,
    name: displayName(fileName, index),
    mimeType: blob.type as PendingImageInput['mimeType'],
    byteLength: blob.size
  };
}

function displayName(value: string, index: number) {
  const name = String(value || '').trim().slice(0, 120);
  return name || `图片 ${index + 1}`;
}

function randomId() {
  if (typeof globalThis.crypto?.randomUUID === 'function') return globalThis.crypto.randomUUID();
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return [...bytes].map((value) => value.toString(16).padStart(2, '0')).join('');
}

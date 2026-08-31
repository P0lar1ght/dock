import type { TransientImagePreview } from '../image-inputs/types.js';
import type { SessionImageAttachment } from './MessageAttachmentModel.js';
import type { SessionState } from './SessionState.js';

interface StoredPreview extends Omit<TransientImagePreview, 'blob'> {
  previewUrl: string;
}

const MAX_TRANSIENT_TURNS = 4;

/**
 * Keeps a small, page-lifetime view of the exact normalized screenshot sent
 * for a Turn. Blob URLs never enter protocol messages or browser storage.
 */
export class TransientScreenshotPreviewStore {
  private readonly previews = new Map<string, readonly StoredPreview[]>();

  add(
    turnId: string,
    value: TransientImagePreview | readonly TransientImagePreview[]
  ) {
    const id = String(turnId || '').trim();
    if (!id || typeof globalThis.URL?.createObjectURL !== 'function') return;
    this.remove(id);
    const previews = Array.isArray(value) ? value : [value];
    this.previews.set(id, previews.slice(0, 4).map((preview) => ({
      previewUrl: globalThis.URL.createObjectURL(preview.blob),
      source: preview.source || 'screenshot',
      name: preview.name,
      mimeType: preview.mimeType,
      width: preview.width,
      height: preview.height,
      byteLength: preview.byteLength
    })));
    while (this.previews.size > MAX_TRANSIENT_TURNS) {
      const oldest = this.previews.keys().next().value;
      if (typeof oldest !== 'string') break;
      this.remove(oldest);
    }
  }

  project(state: SessionState): SessionState {
    let changed = false;
    const messages = state.messages.map((message) => {
      let attachmentChanged = false;
      const previews = this.previews.get(message.turnId);
      const attachments = message.attachments?.map((attachment, index) => {
        const preview = previews?.[index];
        if (!preview && !attachment.previewUrl) return attachment;
        const next: SessionImageAttachment = preview
          ? {
              ...attachment,
              ...preview,
              type: preview.source === 'screenshot' ? 'screenshot' : 'image'
            }
          : withoutPreview(attachment);
        if (samePreview(attachment, next)) return attachment;
        changed = true;
        attachmentChanged = true;
        return next;
      });
      return attachmentChanged ? { ...message, attachments } : message;
    });
    return changed ? { ...state, messages } : state;
  }

  clear() {
    for (const turnId of [...this.previews.keys()]) this.remove(turnId);
  }

  private remove(turnId: string) {
    const current = this.previews.get(turnId) || [];
    for (const preview of current) globalThis.URL.revokeObjectURL(preview.previewUrl);
    this.previews.delete(turnId);
  }
}

function withoutPreview(attachment: SessionImageAttachment): SessionImageAttachment {
  const { previewUrl: _previewUrl, name: _name, ...safe } = attachment;
  return safe;
}

function samePreview(
  current: SessionImageAttachment,
  next: SessionImageAttachment
) {
  return current.previewUrl === next.previewUrl
    && current.type === next.type
    && current.name === next.name
    && current.width === next.width
    && current.height === next.height
    && current.byteLength === next.byteLength;
}

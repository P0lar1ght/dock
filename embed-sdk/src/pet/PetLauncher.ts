import type {
  NormalizedPetPosition,
  PetLauncherCallbacks,
  PetPosition,
  PetSize,
  PetViewport
} from './types.js';

export interface PetLauncherOptions {
  application: string;
  margin?: number;
  dragThreshold?: number;
  storage?: Storage;
  viewport?: () => PetViewport;
}

export interface PetPanelPlacement extends PetPosition {
  side: 'left' | 'right';
  anchorY: number;
}

interface DragSession {
  pointerId: number;
  startPointer: PetPosition;
  startPosition: PetPosition;
  dragged: boolean;
  lastX: number;
}

export class PetLauncher {
  private readonly margin: number;
  private readonly threshold: number;
  private readonly storage?: Storage;
  private readonly viewport: () => PetViewport;
  private position: PetPosition = { x: 0, y: 0 };
  private drag?: DragSession;
  private suppressClick = false;

  constructor(
    private readonly wrapper: HTMLElement,
    private readonly button: HTMLElement,
    private readonly callbacks: PetLauncherCallbacks,
    private readonly options: PetLauncherOptions
  ) {
    this.margin = options.margin ?? 12;
    this.threshold = options.dragThreshold ?? 6;
    this.storage = options.storage ?? safeLocalStorage();
    this.viewport = options.viewport || currentViewport;
    this.button.addEventListener('pointerdown', this.pointerDown);
    this.button.addEventListener('pointermove', this.pointerMove);
    this.button.addEventListener('pointerup', this.pointerUp);
    this.button.addEventListener('pointercancel', this.pointerCancel);
    this.button.addEventListener('click', this.click);
    globalThis.addEventListener?.('resize', this.resize);
    this.restore();
  }

  get currentPosition() {
    return { ...this.position };
  }

  moveTo(position: PetPosition, persist = false) {
    const clamped = clampPetPosition(position, this.petSize(), this.viewport(), this.margin);
    this.position = clamped;
    this.wrapper.style.left = `${clamped.x}px`;
    this.wrapper.style.top = `${clamped.y}px`;
    this.wrapper.style.right = 'auto';
    this.wrapper.style.bottom = 'auto';
    this.callbacks.onPosition?.(clamped);
    if (persist) this.persist();
  }

  panelPlacement(panel: PetSize): PetPanelPlacement {
    return placePetPanel(this.position, this.petSize(), panel, this.viewport(), this.margin);
  }

  panelPosition(panel: PetSize): PetPosition {
    const { x, y } = this.panelPlacement(panel);
    return { x, y };
  }

  destroy() {
    this.button.removeEventListener('pointerdown', this.pointerDown);
    this.button.removeEventListener('pointermove', this.pointerMove);
    this.button.removeEventListener('pointerup', this.pointerUp);
    this.button.removeEventListener('pointercancel', this.pointerCancel);
    this.button.removeEventListener('click', this.click);
    globalThis.removeEventListener?.('resize', this.resize);
  }

  private readonly pointerDown = (event: PointerEvent) => {
    if (event.button !== 0 && event.pointerType === 'mouse') return;
    this.drag = {
      pointerId: event.pointerId,
      startPointer: { x: event.clientX, y: event.clientY },
      startPosition: this.position,
      dragged: false,
      lastX: event.clientX
    };
    this.button.setPointerCapture?.(event.pointerId);
    this.button.dataset.dragging = 'pending';
  };

  private readonly pointerMove = (event: PointerEvent) => {
    const drag = this.drag;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dx = event.clientX - drag.startPointer.x;
    const dy = event.clientY - drag.startPointer.y;
    const direction = event.clientX < drag.lastX ? 'left' : 'right';
    drag.lastX = event.clientX;
    if (!drag.dragged && Math.hypot(dx, dy) >= this.threshold) {
      drag.dragged = true;
      this.button.dataset.dragging = 'true';
      this.callbacks.onDragStart?.(direction);
    }
    if (!drag.dragged) return;
    event.preventDefault();
    this.moveTo({ x: drag.startPosition.x + dx, y: drag.startPosition.y + dy });
    this.callbacks.onDragMove?.(direction);
  };

  private readonly pointerUp = (event: PointerEvent) => {
    const drag = this.drag;
    if (!drag || drag.pointerId !== event.pointerId) return;
    this.button.releasePointerCapture?.(event.pointerId);
    this.drag = undefined;
    delete this.button.dataset.dragging;
    if (drag.dragged) {
      this.suppressClick = true;
      this.persist();
      this.callbacks.onDragEnd?.();
    }
  };

  private readonly pointerCancel = (event: PointerEvent) => {
    if (!this.drag || this.drag.pointerId !== event.pointerId) return;
    const dragged = this.drag.dragged;
    this.drag = undefined;
    delete this.button.dataset.dragging;
    if (dragged) this.callbacks.onDragEnd?.();
  };

  private readonly click = (event: MouseEvent) => {
    if (this.suppressClick) {
      event.preventDefault();
      event.stopPropagation();
      this.suppressClick = false;
      return;
    }
    this.callbacks.onActivate();
  };

  private readonly resize = () => this.moveTo(this.position, true);

  private restore() {
    const size = this.petSize();
    const viewport = this.viewport();
    const stored = readPosition(this.storage, this.storageKey());
    const position = stored
      ? denormalizePetPosition(stored, size, viewport, this.margin)
      : { x: viewport.width - size.width - this.margin, y: viewport.height - size.height - this.margin };
    this.moveTo(position);
  }

  private persist() {
    const normalized = normalizePetPosition(this.position, this.petSize(), this.viewport(), this.margin);
    try {
      this.storage?.setItem(this.storageKey(), JSON.stringify(normalized));
    } catch {
      // UI coordinates are an optional preference; storage failure is non-fatal.
    }
  }

  private storageKey() {
    const origin = typeof location === 'undefined' ? 'unknown-origin' : location.origin;
    return `dock:pet-position:${this.options.application}:${origin}`;
  }

  private petSize(): PetSize {
    const bounds = this.wrapper.getBoundingClientRect();
    return { width: bounds.width || 112, height: bounds.height || 122 };
  }
}

export function clampPetPosition(position: PetPosition, size: PetSize, viewport: PetViewport, margin = 12) {
  return {
    x: clamp(position.x, margin, Math.max(margin, viewport.width - size.width - margin)),
    y: clamp(position.y, margin, Math.max(margin, viewport.height - size.height - margin))
  };
}

export function placePetPanel(
  position: PetPosition,
  pet: PetSize,
  panel: PetSize,
  viewport: PetViewport,
  margin = 12,
  gap = 14
): PetPanelPlacement {
  const opensLeft = position.x + pet.width / 2 > viewport.width / 2;
  const requested = {
    x: opensLeft ? position.x - panel.width - gap : position.x + pet.width + gap,
    y: position.y + pet.height / 2 - panel.height / 2
  };
  const clampedPosition = clampPetPosition(requested, panel, viewport, margin);
  return {
    ...clampedPosition,
    side: opensLeft ? 'left' : 'right',
    anchorY: clamp(position.y + pet.height / 2 - clampedPosition.y, 24, Math.max(24, panel.height - 24))
  };
}

export function normalizePetPosition(position: PetPosition, size: PetSize, viewport: PetViewport, margin = 12) {
  const availableX = Math.max(1, viewport.width - size.width - margin * 2);
  const availableY = Math.max(1, viewport.height - size.height - margin * 2);
  return { x: clamp((position.x - margin) / availableX, 0, 1), y: clamp((position.y - margin) / availableY, 0, 1) };
}

export function denormalizePetPosition(position: NormalizedPetPosition, size: PetSize, viewport: PetViewport, margin = 12) {
  return clampPetPosition({
    x: margin + clamp(position.x, 0, 1) * Math.max(1, viewport.width - size.width - margin * 2),
    y: margin + clamp(position.y, 0, 1) * Math.max(1, viewport.height - size.height - margin * 2)
  }, size, viewport, margin);
}

function readPosition(storage: Storage | undefined, key: string): NormalizedPetPosition | undefined {
  try {
    const value = JSON.parse(storage?.getItem(key) || 'null');
    if (value && Number.isFinite(value.x) && Number.isFinite(value.y)) return value;
  } catch {
    // Ignore malformed or inaccessible preference storage.
  }
  return undefined;
}

function currentViewport() {
  return { width: globalThis.innerWidth || 1024, height: globalThis.innerHeight || 768 };
}

function safeLocalStorage() {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

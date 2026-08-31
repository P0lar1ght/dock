import type {
  LoadedPetSkin,
  PetAnimationDefinition,
  PetAnimatorOptions,
  PetVisualState
} from './types.js';

export class PetAnimator {
  private skin?: LoadedPetSkin;
  private state: PetVisualState = 'idle';
  private animation?: PetAnimationDefinition;
  private frame = 0;
  private startedAt = 0;
  private handle?: number;
  private readonly reducedMotion: boolean;
  private readonly requestFrame: (callback: FrameRequestCallback) => number;
  private readonly cancelFrame: (handle: number) => void;
  private readonly now: () => number;

  constructor(private readonly element: HTMLElement, private readonly options: PetAnimatorOptions = {}) {
    this.reducedMotion = options.reducedMotion ?? globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
    this.requestFrame = options.requestFrame || ((callback) => requestAnimationFrame(callback));
    this.cancelFrame = options.cancelFrame || ((handle) => cancelAnimationFrame(handle));
    this.now = options.now || (() => performance.now());
  }

  get currentState() {
    return this.state;
  }

  setSkin(skin: LoadedPetSkin) {
    this.skin = skin;
    this.element.style.backgroundImage = `url("${cssUrl(skin.atlasUrl)}")`;
    this.element.style.backgroundSize = `${skin.manifest.atlas.columns * 100}% ${skin.manifest.atlas.rows * 100}%`;
    this.element.dataset.skin = skin.manifest.id;
    this.setState(this.state, true);
  }

  setState(state: PetVisualState, restart = false) {
    if (!restart && this.state === state && this.animation) return;
    this.state = state;
    this.animation = this.skin?.manifest.animations[state];
    this.frame = 0;
    this.startedAt = this.now();
    this.element.dataset.state = state;
    this.stop();
    this.renderFrame();
    if (!this.reducedMotion && this.animation && this.animation.frames > 1) {
      this.handle = this.requestFrame((time) => this.tick(time));
    }
  }

  destroy() {
    this.stop();
    this.skin = undefined;
    this.element.style.backgroundImage = '';
  }

  private tick(time: number) {
    const animation = this.animation;
    if (!animation) return;
    const elapsedFrames = Math.floor(((time - this.startedAt) / 1000) * animation.fps);
    if (!animation.loop && elapsedFrames >= animation.frames) {
      this.frame = animation.frames - 1;
      this.renderFrame();
      this.options.onSettled?.(this.state);
      if (this.state !== 'error' && this.state !== 'sleeping') this.setState('idle', true);
      return;
    }
    const nextFrame = animation.loop ? elapsedFrames % animation.frames : Math.min(elapsedFrames, animation.frames - 1);
    if (this.frame !== nextFrame) {
      this.frame = nextFrame;
      this.renderFrame();
    }
    this.handle = this.requestFrame((nextTime) => this.tick(nextTime));
  }

  private renderFrame() {
    const skin = this.skin;
    const animation = this.animation;
    if (!skin || !animation) return;
    const { columns, rows } = skin.manifest.atlas;
    const x = columns > 1 ? (this.frame / (columns - 1)) * 100 : 0;
    const y = rows > 1 ? (animation.row / (rows - 1)) * 100 : 0;
    this.element.style.backgroundPosition = `${x}% ${y}%`;
  }

  private stop() {
    if (this.handle !== undefined) this.cancelFrame(this.handle);
    this.handle = undefined;
  }
}

function cssUrl(value: string) {
  return value.replace(/["\\\n\r]/g, (match) => `\\${match}`);
}

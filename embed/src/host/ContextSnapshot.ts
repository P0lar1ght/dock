import type { NormalizedContextItem, TurnContextEnvelope } from '../protocol/context.js';

export class ContextSnapshot implements TurnContextEnvelope {
  readonly contextItems: readonly NormalizedContextItem[];
  readonly contextSources: readonly string[];

  constructor(items: readonly NormalizedContextItem[], sources: readonly string[]) {
    this.contextItems = deepFreeze(structuredClone(items));
    this.contextSources = Object.freeze([...new Set(sources)].sort());
    Object.freeze(this);
  }
}

function deepFreeze<T>(value: T): T {
  if (value && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const item of Object.values(value as Record<string, unknown>)) deepFreeze(item);
    Object.freeze(value);
  }
  return value;
}

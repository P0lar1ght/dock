import { DockAgentElement } from './DockAgentElement.js';

export const DOCK_AGENT_TAG = 'dock-agent';

export function defineDockAgent(registry: CustomElementRegistry = customElements) {
  const existing = registry.get(DOCK_AGENT_TAG);
  if (existing) return existing;
  registry.define(DOCK_AGENT_TAG, DockAgentElement);
  return DockAgentElement;
}

import { applyAgentOptions, type DockElementOptions } from './attributes.js';
import { defineDockAgent, DOCK_AGENT_TAG } from './defineElement.js';
import type { DockAgentElement } from './DockAgentElement.js';

export interface MountDockOptions extends DockElementOptions {
  target?: ParentNode;
}

export function mountDock(options: MountDockOptions) {
  defineDockAgent();
  const target = options.target || document.body;
  if (!target) throw new Error('Dock requires a document body or explicit mount target');
  const element = document.createElement(DOCK_AGENT_TAG) as DockAgentElement;
  applyAgentOptions(element, options);
  target.appendChild(element);
  return element;
}

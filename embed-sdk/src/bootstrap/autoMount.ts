import { mountDock } from '../element/mount.js';
import { findAutoMountScript, scriptAgentOptions } from './scriptAttributes.js';

export function autoMountDock(documentValue: Document = document) {
  const script = findAutoMountScript(documentValue);
  if (!script) return undefined;
  if (script.dataset.dockMounted === 'true') return undefined;
  script.dataset.dockMounted = 'true';

  const mount = () => {
    const options = scriptAgentOptions(script);
    const selector = `dock-agent[data-dock-auto="true"][application="${cssEscape(options.application)}"]`;
    const existing = documentValue.querySelector(selector);
    if (existing) return existing;
    const element = mountDock({ ...options, target: documentValue.body });
    element.dataset.dockAuto = 'true';
    return element;
  };

  if (documentValue.body) return mount();
  documentValue.addEventListener('DOMContentLoaded', mount, { once: true });
  return undefined;
}

function cssEscape(value: string) {
  return globalThis.CSS?.escape ? globalThis.CSS.escape(value) : value.replace(/[^a-z0-9_-]/gi, '\\$&');
}

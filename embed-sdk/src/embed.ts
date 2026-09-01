import { autoMountDock } from './bootstrap/autoMount.js';

export { DOCK_EMBED_VERSION } from './version.js';
export { autoMountDock } from './bootstrap/autoMount.js';
export { defineDockAgent, DOCK_AGENT_TAG } from './element/defineElement.js';
export { mountDock, type MountDockOptions } from './element/mount.js';
export { DockAgentElement } from './element/DockAgentElement.js';
export type { DockAgentPublicApi } from './element/publicApi.js';

if (typeof document !== 'undefined') autoMountDock(document);

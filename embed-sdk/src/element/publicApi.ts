import type { DockClient } from '../client/DockClient.js';
import type { LoadedPetSkin, PetSkinSummary } from '../pet/types.js';

export interface DockAgentPublicApi {
  readonly client?: DockClient;
  readonly open: boolean;
  connect(): Promise<void>;
  openChat(): Promise<void>;
  closeChat(): void;
  toggleChat(): Promise<void>;
  setSkin(id: string): Promise<LoadedPetSkin>;
  importSkin(file: File): Promise<LoadedPetSkin>;
  listSkins(): PetSkinSummary[];
}

export interface DockAgentReadyDetail {
  application: string;
  gatewayUrl: string;
}

export interface DockAgentStateDetail {
  visual: string;
  label: string;
  live: boolean;
}

export const DOCK_AGENT_READY = 'dock:ready';
export const DOCK_AGENT_STATE = 'dock:state';
export const DOCK_AGENT_TOGGLE = 'dock:toggle';
export const DOCK_AGENT_ERROR = 'dock:error';

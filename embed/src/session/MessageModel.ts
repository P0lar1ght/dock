import type { SessionMessageAttachment } from './MessageAttachmentModel.js';

export type SessionMessageRole = 'user' | 'assistant';

export interface SessionMessage {
  id: string;
  turnId: string;
  role: SessionMessageRole;
  content: string;
  attachments?: readonly SessionMessageAttachment[];
  status: 'streaming' | 'completed' | 'failed' | 'cancelled';
  /** True after a tool / permission / ask split. Later tokens start a new bubble. */
  closed?: boolean;
  startedSeq?: number;
  updatedSeq?: number;
}

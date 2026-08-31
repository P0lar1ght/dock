import type { SessionMessageAttachment } from './MessageAttachmentModel.js';

export type SessionMessageRole = 'user' | 'assistant';

export interface SessionMessage {
  id: string;
  turnId: string;
  role: SessionMessageRole;
  content: string;
  attachments?: readonly SessionMessageAttachment[];
  status: 'streaming' | 'completed' | 'failed' | 'cancelled';
  startedSeq?: number;
  updatedSeq?: number;
}

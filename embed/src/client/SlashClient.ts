import { SLASH_EXECUTE, SLASH_LIST } from '../protocol/methods.js';
import type { SlashExecuteResult, SlashListResult } from '../protocol/responses.js';

type Request = <T>(method: string, params?: Record<string, unknown>) => Promise<T>;

export class SlashClient {
  constructor(private readonly request: Request) {}

  list() {
    return this.request<SlashListResult>(SLASH_LIST, {});
  }

  execute(text: string, threadId?: string) {
    // Gateway currently has one live thread. `threadId` must be "live" or omitted;
    // anything else is rejected. User/assistant text after `submitted` arrives
    // on the existing `thread/subscribe` stream.
    return this.request<SlashExecuteResult>(SLASH_EXECUTE, {
      text,
      ...(threadId ? { threadId } : {})
    });
  }
}

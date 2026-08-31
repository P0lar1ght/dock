import { CONTEXT_UPDATE } from '../protocol/methods.js';
import type { ContextUpdateRequest, NormalizedContextItem } from '../protocol/context.js';
import type { RpcRequest } from './ThreadClient.js';

export class ContextClient {
  constructor(private readonly request: RpcRequest) {}

  replace(
    threadId: string,
    workspaceId: string,
    source: string,
    items: readonly NormalizedContextItem[]
  ) {
    return this.update({ threadId, workspaceId, mode: 'replace', source, items });
  }

  remove(threadId: string, workspaceId: string, source: string) {
    return this.update({ threadId, workspaceId, mode: 'remove', source });
  }

  private update(input: ContextUpdateRequest) {
    return this.request<{ ok: true }>(CONTEXT_UPDATE, { ...input, items: input.items ? [...input.items] : undefined });
  }
}

import type {
  PermissionResolutionRequest,
  PermissionResolutionResult
} from '../protocol/permissions.js';

type Request = <T>(method: string, params?: Record<string, unknown>) => Promise<T>;

export class PermissionClient {
  constructor(private readonly request: Request) {}

  resolve(input: PermissionResolutionRequest) {
    return this.request<PermissionResolutionResult>('permission/resolve', {
      requestId: input.requestId,
      threadId: input.threadId,
      turnId: input.turnId,
      workspaceId: input.workspaceId,
      decision: input.decision,
      reason: input.decision === 'approve'
        ? 'Approved in embedded Chat'
        : 'Denied in embedded Chat',
      always: false
    });
  }
}

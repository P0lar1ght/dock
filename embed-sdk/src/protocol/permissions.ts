export type PermissionDecision = 'approve' | 'deny';

export interface PermissionResolutionRequest {
  requestId: string;
  threadId: string;
  turnId: string;
  workspaceId: string;
  decision: PermissionDecision;
}

export interface PermissionResolutionResult {
  ok: boolean;
  resumed: boolean;
  code?: string;
  message?: string;
}

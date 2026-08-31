import type { PermissionDecision } from '../protocol/permissions.js';
import type { AgentSession } from '../session/AgentSession.js';

export interface PermissionInteractionState {
  resolving: boolean;
  error?: string;
}

export class PermissionController {
  private session?: AgentSession;
  private readonly resolving = new Set<string>();
  private readonly errors = new Map<string, string>();

  constructor(private readonly onChange: () => void) {}

  bind(session: AgentSession | undefined) {
    if (this.session === session) return;
    this.session = session;
    this.resolving.clear();
    this.errors.clear();
  }

  get interactions(): Readonly<Record<string, PermissionInteractionState>> {
    const ids = new Set([...this.resolving, ...this.errors.keys()]);
    return Object.fromEntries([...ids].map((id) => [id, {
      resolving: this.resolving.has(id),
      error: this.errors.get(id)
    }]));
  }

  async resolve(requestId: string, decision: PermissionDecision) {
    const session = this.session;
    const request = session?.state.permissionRequests.find((item) => item.id === requestId);
    if (!session || request?.status !== 'pending' || this.resolving.has(requestId)) return false;
    this.resolving.add(requestId);
    this.errors.delete(requestId);
    this.onChange();
    try {
      const result = await session.resolvePermission(requestId, decision);
      if (!result.ok && session.state.permissionRequests.find((item) => item.id === requestId)?.status === 'pending') {
        this.errors.set(requestId, result.message || 'Gateway 未能处理审批决定');
      }
      return result.ok;
    } catch (error) {
      if (session.state.permissionRequests.find((item) => item.id === requestId)?.status === 'pending') {
        this.errors.set(requestId, error instanceof Error ? error.message : '审批请求失败');
      }
      return false;
    } finally {
      this.resolving.delete(requestId);
      this.onChange();
    }
  }

  destroy() {
    this.bind(undefined);
  }
}

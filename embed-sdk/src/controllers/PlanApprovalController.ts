import type { AgentSession } from '../session/AgentSession.js';
import type { PlanDecision } from '../session/PlanApprovalModel.js';

export interface PlanApprovalInteractionState {
  resolving: boolean;
  error?: string;
}

/** 计划审批的提交状态（同 PermissionController：一条一条地记「提交中 / 失败原因」）。 */
export class PlanApprovalController {
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

  get interactions(): Readonly<Record<string, PlanApprovalInteractionState>> {
    const ids = new Set([...this.resolving, ...this.errors.keys()]);
    return Object.fromEntries([...ids].map((id) => [id, {
      resolving: this.resolving.has(id),
      error: this.errors.get(id)
    }]));
  }

  async resolve(planId: string, decision: PlanDecision) {
    const session = this.session;
    const pending = () => session?.state.planApprovals.find((item) => item.id === planId)?.status === 'pending';
    if (!session || !pending() || this.resolving.has(planId)) return false;
    this.resolving.add(planId);
    this.errors.delete(planId);
    this.onChange();
    try {
      const result = await session.resolvePlan(planId, decision);
      if (!result.ok && pending()) this.errors.set(planId, 'Gateway 未能处理这个决定');
      return result.ok;
    } catch (error) {
      if (pending()) this.errors.set(planId, error instanceof Error ? error.message : '提交失败，请重试');
      return false;
    } finally {
      this.resolving.delete(planId);
      this.onChange();
    }
  }

  destroy() {
    this.bind(undefined);
  }
}

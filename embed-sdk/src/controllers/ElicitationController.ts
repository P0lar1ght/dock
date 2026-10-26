import type { AgentSession } from '../session/AgentSession.js';
import type { ElicitAction } from '../session/ElicitationModel.js';

export interface ElicitationInteractionState {
  resolving: boolean;
  error?: string;
}

/** MCP elicitation 的提交状态（同 PlanApprovalController）。 */
export class ElicitationController {
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

  get interactions(): Readonly<Record<string, ElicitationInteractionState>> {
    const ids = new Set([...this.resolving, ...this.errors.keys()]);
    return Object.fromEntries([...ids].map((id) => [id, {
      resolving: this.resolving.has(id),
      error: this.errors.get(id)
    }]));
  }

  async resolve(elicitId: string, action: ElicitAction) {
    const session = this.session;
    const pending = () => session?.state.elicitations.find((item) => item.id === elicitId)?.status === 'pending';
    if (!session || !pending() || this.resolving.has(elicitId)) return false;
    this.resolving.add(elicitId);
    this.errors.delete(elicitId);
    this.onChange();
    try {
      const result = await session.resolveElicitation(elicitId, action);
      if (!result.ok && pending()) this.errors.set(elicitId, 'Gateway 未能处理这个回应');
      return result.ok;
    } catch (error) {
      if (pending()) this.errors.set(elicitId, error instanceof Error ? error.message : '提交失败，请重试');
      return false;
    } finally {
      this.resolving.delete(elicitId);
      this.onChange();
    }
  }

  destroy() {
    this.bind(undefined);
  }
}

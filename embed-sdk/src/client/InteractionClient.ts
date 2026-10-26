import { ELICIT_RESOLVE, INTERACTION_RESPOND, PLAN_RESOLVE } from '../protocol/methods.js';
import type { ElicitAction } from '../session/ElicitationModel.js';
import type { PlanDecision } from '../session/PlanApprovalModel.js';
import type {
  RuntimeInteractionResolutionResult,
  RuntimeUserInputAnswer
} from '../protocol/interactions.js';

type Request = <T>(method: string, params?: Record<string, unknown>) => Promise<T>;

export class InteractionClient {
  constructor(private readonly request: Request) {}

  respond(input: {
    interactionId: string;
    threadId: string;
    turnId: string;
    workspaceId: string;
    answers: RuntimeUserInputAnswer[];
  }) {
    return this.request<RuntimeInteractionResolutionResult>(INTERACTION_RESPOND, {
      interactionId: input.interactionId,
      threadId: input.threadId,
      turnId: input.turnId,
      workspaceId: input.workspaceId,
      answers: input.answers
    });
  }

  /** 计划审批：批准并执行 / 要求修改 / 退出计划模式。 */
  resolvePlan(input: { threadId: string; workspaceId: string; planId: string; decision: PlanDecision }) {
    return this.request<{ ok: boolean; resumed?: boolean }>(PLAN_RESOLVE, {
      threadId: input.threadId,
      workspaceId: input.workspaceId,
      planId: input.planId,
      decision: input.decision
    });
  }

  /** MCP elicitation：接受 / 拒绝 / 取消（解的是这个线程那一页队首的那条）。 */
  resolveElicitation(input: { threadId: string; workspaceId: string; elicitId: string; action: ElicitAction }) {
    return this.request<{ ok: boolean; resumed?: boolean }>(ELICIT_RESOLVE, {
      threadId: input.threadId,
      workspaceId: input.workspaceId,
      elicitId: input.elicitId,
      action: input.action
    });
  }
}

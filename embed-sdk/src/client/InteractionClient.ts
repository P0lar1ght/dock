import { ELICIT_OPEN, ELICIT_RESOLVE, INTERACTION_RESPOND, PLAN_RESOLVE } from '../protocol/methods.js';
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

  /**
   * MCP elicitation 的 URL 模式：开浏览器。
   *
   * 网关按提问自己的 `reply_on_consent` 决定要不要顺手回服务器「用户同意」：
   * - server-initiated（服务器发 `elicitation/create`）：回 —— 规范要求用户同意后
   *   客户端立刻回 `{"action":"accept"}`，不等 `complete` 通知（那是 MAY）。
   * - `-32042`（服务器用错误码要 URL 交互）：不回 —— 那里没有挂着的 JSON-RPC
   *   请求要回，等 `complete` 通知到了才该重试原请求。
   *
   * `consented` 回的是网关有没有替用户回那一下。
   */
  openElicitation(input: { threadId: string; workspaceId: string; elicitId: string }) {
    return this.request<{ ok: boolean; consented: boolean }>(ELICIT_OPEN, {
      threadId: input.threadId,
      workspaceId: input.workspaceId,
      elicitId: input.elicitId
    });
  }
}

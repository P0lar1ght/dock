import type { ReasoningEffort } from '../protocol/responses.js';
import type { AgentSession } from '../session/AgentSession.js';

export interface ThreadMemorySelection {
  read?: boolean;
  write?: boolean;
}

type SessionMutation = (session: AgentSession) => Promise<unknown>;

/** Owns the compact execution controls shown inside the Composer. */
export class ThreadExecutionControlController {
  private sessionValue?: AgentSession;
  private changingValue = false;
  private errorValue = '';

  constructor(private readonly onChange: () => void) {}

  get changing() {
    return this.changingValue;
  }

  get error() {
    return this.errorValue;
  }

  bind(session: AgentSession | undefined) {
    if (this.sessionValue === session) return;
    this.sessionValue = session;
    this.changingValue = false;
    this.errorValue = '';
  }

  clearError() {
    if (!this.errorValue) return;
    this.errorValue = '';
    this.onChange();
  }

  selectReasoning(effort: ReasoningEffort) {
    return this.mutate(
      (session) => session.setReasoning(effort),
      '推理强度未能切换，请检查当前模型能力'
    );
  }

  selectPlanMode(enabled: boolean) {
    return this.mutate(
      (session) => session.setPlanMode(enabled),
      '计划模式未能切换，请刷新 Thread 状态后重试'
    );
  }

  selectMemory(selection: ThreadMemorySelection) {
    return this.mutate(
      (session) => session.setMemory(selection),
      '记忆权限未能切换，请检查 Gateway 的 Memory 能力'
    );
  }

  compactContext() {
    return this.mutate(
      async (session) => {
        const result = await session.compactContext();
        if (result.status === 'busy' || result.status === 'running' || result.status === 'skipped') {
          throw new Error(result.failureCategory || result.status);
        }
      },
      '上下文暂时无法压缩，请等待当前回复完成后重试'
    );
  }

  private async mutate(operation: SessionMutation, errorMessage: string) {
    const session = this.sessionValue;
    if (!session || this.changingValue) return false;
    this.changingValue = true;
    this.errorValue = '';
    this.onChange();
    try {
      await operation(session);
      return true;
    } catch {
      this.errorValue = errorMessage;
      return false;
    } finally {
      this.changingValue = false;
      this.onChange();
    }
  }
}

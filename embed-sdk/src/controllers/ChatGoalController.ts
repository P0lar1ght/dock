import type { AgentSession } from '../session/AgentSession.js';
import type { SessionState } from '../session/SessionState.js';
import { cancellableTurn } from './TurnController.js';
import type { GoalCommand } from './GoalCommand.js';

/** Owns Goal command and popover mutations without expanding the chat orchestration class. */
export class ChatGoalController {
  draft = '';
  changing = false;
  error = '';

  constructor(private readonly onChange: () => void) {}

  reset() {
    this.draft = '';
    this.changing = false;
    this.error = '';
  }

  setDraft(value: string) {
    this.draft = value.slice(0, 2_000);
    this.error = '';
    this.onChange();
  }

  save(session: AgentSession | undefined, state: Readonly<SessionState> | undefined) {
    const content = this.draft.trim();
    if (!content) return Promise.resolve(false);
    const goal = state?.environment?.goal;
    const operation = goal && goal.status !== 'none' && goal.status !== 'completed'
      ? () => session!.editGoal(content, goal.revision || 0)
      : () => session!.startGoal(content);
    return this.mutate(session, operation, '目标未能保存，请检查内容后重试', true);
  }

  pause(session: AgentSession | undefined, state: Readonly<SessionState> | undefined) {
    const revision = state?.environment?.goal.revision || 0;
    return this.mutate(
      session,
      () => session!.pauseGoal(revision),
      '目标未能暂停，请刷新 Thread 状态后重试'
    );
  }

  resume(session: AgentSession | undefined, state: Readonly<SessionState> | undefined) {
    const revision = state?.environment?.goal.revision || 0;
    return this.mutate(
      session,
      () => session!.resumeGoal(revision),
      '目标未能恢复，请确认 Thread 空闲后重试'
    );
  }

  clear(session: AgentSession | undefined) {
    return this.mutate(
      session,
      () => session!.clearGoal(),
      '目标未能清除，请刷新 Thread 状态后重试',
      true
    );
  }

  async execute(
    session: AgentSession,
    command: Exclude<GoalCommand, { action: 'show' }>
  ) {
    const goal = session.state.environment?.goal;
    const revision = goal?.revision || 0;
    if (command.action === 'start') {
      if (cancellableTurn(session.state.turns)) throw new Error('新 Goal 只能在空闲 Thread 中启动');
      await session.startGoal(command.content);
    } else if (command.action === 'edit') {
      if (!goal || goal.status === 'none') throw new Error('当前 Thread 尚未设置 Goal');
      await session.editGoal(command.content, revision);
    } else if (command.action === 'pause') {
      if (goal?.status !== 'active') throw new Error('只有 active Goal 可以暂停');
      await session.pauseGoal(revision);
    } else if (command.action === 'resume') {
      if (goal?.status !== 'paused' && goal?.status !== 'blocked') {
        throw new Error('只有 paused 或 blocked Goal 可以恢复');
      }
      if (cancellableTurn(session.state.turns)) throw new Error('恢复 Goal 需要空闲 Thread');
      await session.resumeGoal(revision);
    } else {
      if (!goal || goal.status === 'none') throw new Error('当前 Thread 尚未设置 Goal');
      await session.clearGoal();
    }
    await session.refreshEnvironment().catch(() => undefined);
  }

  private async mutate(
    session: AgentSession | undefined,
    operation: () => Promise<unknown>,
    errorMessage: string,
    clearDraft = false
  ) {
    if (!session || this.changing) return false;
    this.changing = true;
    this.error = '';
    this.onChange();
    try {
      await operation();
      if (clearDraft) this.draft = '';
      return true;
    } catch {
      this.error = errorMessage;
      return false;
    } finally {
      this.changing = false;
      this.onChange();
    }
  }
}

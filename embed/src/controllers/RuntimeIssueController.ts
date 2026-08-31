import type { AgentSession } from '../session/AgentSession.js';
import type { SessionMessage } from '../session/MessageModel.js';
import type { SessionRuntimeIssue } from '../session/RuntimeIssueModel.js';

export interface RuntimeIssueInteraction {
  retrying: boolean;
  error?: string;
}

export class RuntimeIssueController {
  private session?: AgentSession;
  private readonly dismissed = new Set<string>();
  private readonly retrying = new Set<string>();
  private readonly errors = new Map<string, string>();

  constructor(
    private readonly onChange: () => void,
    private readonly editDraft: (value: string) => void
  ) {}

  bind(session: AgentSession | undefined) {
    this.session = session;
  }

  visible(issues: readonly SessionRuntimeIssue[], messages: readonly SessionMessage[]) {
    return issues.filter((issue) =>
      !this.dismissed.has(issue.id)
      && !messages.some((message) =>
        message.role === 'user' && (message.startedSeq || 0) > issue.seq
      )
    );
  }

  interactions(issues: readonly SessionRuntimeIssue[]) {
    return Object.fromEntries(issues.map((issue) => [issue.id, {
      retrying: this.retrying.has(issue.id),
      error: this.errors.get(issue.id)
    }]));
  }

  async retry(issue: SessionRuntimeIssue) {
    const session = this.session;
    if (!session || this.retrying.has(issue.id)) return false;
    this.retrying.add(issue.id);
    this.errors.delete(issue.id);
    this.onChange();
    try {
      await session.retryTurn(issue.turnId);
      this.dismissed.add(issue.id);
      return true;
    } catch {
      this.errors.set(issue.id, '重试未被 Gateway 接受，请检查连接后再试');
      return false;
    } finally {
      this.retrying.delete(issue.id);
      this.onChange();
    }
  }

  edit(issue: SessionRuntimeIssue) {
    const message = this.session?.messageForTurn(issue.turnId);
    if (!message) {
      this.errors.set(issue.id, '找不到原始用户消息');
      this.onChange();
      return false;
    }
    this.editDraft(message);
    this.dismiss(issue.id);
    return true;
  }

  dismiss(issueId: string) {
    this.dismissed.add(issueId);
    this.errors.delete(issueId);
    this.retrying.delete(issueId);
    this.onChange();
  }

  destroy() {
    this.session = undefined;
    this.dismissed.clear();
    this.retrying.clear();
    this.errors.clear();
  }
}

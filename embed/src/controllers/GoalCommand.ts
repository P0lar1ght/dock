export type GoalCommand =
  | { action: 'show' }
  | { action: 'start'; content: string }
  | { action: 'edit'; content: string }
  | { action: 'pause' | 'resume' | 'clear' };

export function parseGoalCommand(value: string): GoalCommand | undefined {
  const match = /^\/goal(?:\s+([\s\S]+))?$/u.exec(String(value || '').trim());
  if (!match) return undefined;
  const argument = String(match[1] || '').trim();
  if (!argument) return { action: 'show' };
  const control = /^(edit|pause|resume|clear)(?:\s+([\s\S]+))?$/u.exec(argument);
  if (!control) return { action: 'start', content: argument };
  const action = control[1] as 'edit' | 'pause' | 'resume' | 'clear';
  const content = String(control[2] || '').trim();
  if (action === 'edit') {
    if (!content) throw new Error('/goal edit 后需要填写新的可验证目标');
    return { action, content };
  }
  if (content) throw new Error(`/goal ${action} 不接受额外参数`);
  return { action };
}

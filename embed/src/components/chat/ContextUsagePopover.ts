import { html, nothing } from 'lit';
import { faChevronRight } from '@fortawesome/free-solid-svg-icons/faChevronRight';
import { faCube } from '@fortawesome/free-solid-svg-icons/faCube';
import { faBrain } from '@fortawesome/free-solid-svg-icons/faBrain';
import { faBullseye } from '@fortawesome/free-solid-svg-icons/faBullseye';
import { faListCheck } from '@fortawesome/free-solid-svg-icons/faListCheck';
import { faBookOpen } from '@fortawesome/free-solid-svg-icons/faBookOpen';
import { faCompress } from '@fortawesome/free-solid-svg-icons/faCompress';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';
import { environmentDetails } from './EnvironmentDetails.js';

export function contextUsagePopover(
  environment: SessionEnvironment | undefined,
  syncing = false,
  openModel: () => void = () => undefined,
  openReasoning: () => void = () => undefined,
  openGoal: () => void = () => undefined,
  openPlan: () => void = () => undefined,
  openMemory: () => void = () => undefined,
  activeTurn = false,
  compacting = false,
  compactContext: () => void = () => undefined
) {
  if (!environment) {
    return html`
      <aside class="context-popover" data-testid="context-popover" aria-label="上下文用量">
        <div class="context-popover-heading">
          <strong>上下文用量</strong>
          <span>正在同步</span>
        </div>
        <p class="context-empty">等待 Gateway 返回当前 Thread 的安全状态。</p>
      </aside>
    `;
  }

  const context = environment.context;
  const usage = clampedPercent(context.usagePercent);
  const trigger = clampedPercent(context.compactionTriggerPercent);
  return html`
    <aside class="context-popover" data-testid="context-popover" aria-label="上下文用量">
      <div class="context-popover-heading">
        <strong>上下文用量</strong>
        <span>
          ≈${formatTokens(context.estimatedTokens)} / ${formatTokens(context.maxContextTokens)}
          ${syncing ? ' · 等待同步' : ''}
        </span>
      </div>
      <div
        class="context-meter"
        role="meter"
        aria-label="当前上下文用量"
        aria-valuemin="0"
        aria-valuemax=${String(context.maxContextTokens)}
        aria-valuenow=${String(context.estimatedTokens)}
      >
        <span class="context-meter-fill" style=${`width:${usage}%`}></span>
        ${context.compactionEnabled ? html`
          <span class="context-meter-trigger" style=${`left:${trigger}%`} aria-hidden="true"></span>
        ` : nothing}
        <span class="context-meter-value">${formatPercent(usage)}</span>
      </div>
      <div class="context-meter-labels">
        <span>当前 ≈${formatTokens(context.estimatedTokens)}</span>
        <span>
          ${context.compactionEnabled
            ? `${formatTokens(context.compactionTriggerTokens)} 自动压缩`
            : '自动压缩已关闭'}
        </span>
        <span>最大 ${formatTokens(context.maxContextTokens)}</span>
      </div>
      <dl class="context-facts">
        <div><dt>模型输入预算</dt><dd>${formatTokens(context.inputBudgetTokens)}</dd></div>
        <div><dt>输出预留</dt><dd>${formatTokens(context.reservedOutputTokens)}</dd></div>
      </dl>
      ${compactionSummary(context.lastCompaction)}
      <button
        class="composer-menu-row"
        data-testid="context-compact"
        type="button"
        ?disabled=${!context.canCompact || activeTurn || compacting || context.lastCompaction?.status === 'running'}
        @click=${compactContext}
      >
        ${iconTemplate(faCompress)}
        <span>
          <strong>${context.lastCompaction?.status === 'running' ? '正在压缩' : '压缩上下文'}</strong>
          <small>${compactActionSummary(context.canCompact, activeTurn)}</small>
        </span>
      </button>
      <button class="composer-menu-row" data-testid="context-open-plan" type="button" @click=${openPlan}>
        ${iconTemplate(faListCheck)}
        <span>
          <strong>计划模式</strong>
          <small>${environment.plan.enabled ? '开 · 只读规划' : '关 · 执行模式'}</small>
        </span>
        ${iconTemplate(faChevronRight)}
      </button>
      ${environment.memory.canRead || environment.memory.canWrite ? html`
        <button class="composer-menu-row" data-testid="context-open-memory" type="button" @click=${openMemory}>
          ${iconTemplate(faBookOpen)}
          <span>
            <strong>记忆</strong>
            <small>${memorySummary(environment.memory)}</small>
          </span>
          ${iconTemplate(faChevronRight)}
        </button>
      ` : nothing}
      ${environment.goal.status !== 'none' ? html`
        <button class="composer-menu-row" data-testid="context-open-goal" type="button" @click=${openGoal}>
          ${iconTemplate(faBullseye)}
          <span>
            <strong>目标</strong>
            <small>${goalSummary(environment.goal)}</small>
          </span>
          ${iconTemplate(faChevronRight)}
        </button>
      ` : nothing}
      <button class="composer-menu-row" data-testid="context-open-model" type="button" @click=${openModel}>
        ${iconTemplate(faCube)}
        <span><strong>模型</strong><small>${environment.model.label}</small></span>
        ${iconTemplate(faChevronRight)}
      </button>
      ${environment.reasoning.supported ? html`
        <button class="composer-menu-row" data-testid="context-open-reasoning" type="button" @click=${openReasoning}>
          ${iconTemplate(faBrain)}
          <span><strong>推理</strong><small>${environment.reasoning.effort || '模型默认'}</small></span>
          ${iconTemplate(faChevronRight)}
        </button>
      ` : nothing}
      ${environmentDetails(environment)}
    </aside>
  `;
}

function memorySummary(memory: SessionEnvironment['memory']) {
  if (memory.read && memory.write) return '读取与生成 开';
  if (memory.read) return '读取 开 · 生成 关';
  if (memory.write) return '读取 关 · 生成 开';
  return '关闭';
}

function goalSummary(goal: SessionEnvironment['goal']) {
  if (goal.status === 'active') return goal.summary || '持续追求中';
  if (goal.status === 'paused') return `已暂停 · ${goal.summary}`;
  if (goal.status === 'blocked') return `已阻塞 · ${goal.summary}`;
  if (goal.status === 'completed') return `已完成 · ${goal.summary}`;
  return '设置要持续追求的目标';
}

function compactionSummary(value: SessionEnvironment['context']['lastCompaction']) {
  if (!value) return nothing;
  if (value.status === 'running') {
    return html`<p class="context-compaction" data-status="running">正在压缩当前 Thread…</p>`;
  }
  if (value.status === 'completed') {
    return html`
      <p class="context-compaction" data-status="completed">
        最近压缩 ${formatTokens(value.beforeTokens)} → ${formatTokens(value.afterTokens)}
      </p>
    `;
  }
  if (value.status === 'suppressed') {
    return html`<p class="context-compaction" data-status="suppressed">当前输入暂不重复尝试压缩</p>`;
  }
  if (value.failureCategory === 'interrupted') {
    return html`<p class="context-compaction" data-status="failed">上次压缩因 Gateway 重启中断，可重新尝试</p>`;
  }
  return html`<p class="context-compaction" data-status="failed">最近压缩未完成，当前上下文已保留</p>`;
}

function compactActionSummary(canCompact: boolean, activeTurn: boolean) {
  if (!canCompact) return '当前 Gateway 未启用';
  if (activeTurn) return '当前回复完成后可用';
  return '立即压缩当前 Thread 历史';
}

function clampedPercent(value: number) {
  return Math.min(100, Math.max(0, Number.isFinite(value) ? value : 0));
}

function formatPercent(value: number) {
  return `${Math.round(value)}%`;
}

function formatTokens(value: number) {
  const tokens = Math.max(0, Math.round(value));
  if (tokens >= 1_000_000) return `${trim(tokens / 1_000_000)}m`;
  if (tokens >= 1_000) return `${trim(tokens / 1_000)}k`;
  return String(tokens);
}

function trim(value: number) {
  return value.toFixed(value >= 100 || Number.isInteger(value) ? 0 : 1).replace(/\.0$/, '');
}

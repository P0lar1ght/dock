import { html } from 'lit';
import { faBookOpen } from '@fortawesome/free-solid-svg-icons/faBookOpen';
import { faChevronRight } from '@fortawesome/free-solid-svg-icons/faChevronRight';
import { faPlug } from '@fortawesome/free-solid-svg-icons/faPlug';

import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

export function environmentDetails(environment: SessionEnvironment) {
  return html`
    ${mcpDetails(environment.mcp)}
    ${rulesDetails(environment.rules)}
  `;
}

function mcpDetails(mcp: SessionEnvironment['mcp']) {
  const summary = !mcp.enabled
    ? '已关闭'
    : mcp.totalServers === 0
      ? '未配置 Server'
      : `${mcp.connectedServers}/${mcp.totalServers} 已连接 · ${mcp.toolCount} 工具`;
  return html`
    <details class="environment-detail" data-testid="context-mcp-details">
      <summary class="composer-menu-row">
        ${iconTemplate(faPlug)}
        <span><strong>MCP</strong><small>${summary}</small></span>
        ${iconTemplate(faChevronRight, 'control-icon detail-chevron')}
      </summary>
      <div class="environment-detail-list" role="list" aria-label="MCP Server 状态">
        ${mcp.servers.length ? mcp.servers.map((server) => html`
          <div class="environment-detail-item" role="listitem">
            <i class="environment-status" data-status=${server.status} aria-hidden="true"></i>
            <span><strong>${server.label}</strong><small>${serverStatus(server)}</small></span>
          </div>
        `) : html`<p class="context-empty">当前 Gateway 没有公开可用的 MCP Server。</p>`}
      </div>
    </details>
  `;
}

function rulesDetails(rules: SessionEnvironment['rules']) {
  return html`
    <details class="environment-detail" data-testid="context-rules-details">
      <summary class="composer-menu-row">
        ${iconTemplate(faBookOpen)}
        <span>
          <strong>AGENTS.md</strong>
          <small>${rules.appliedCount ? `${rules.appliedCount} 个规则文件 · 查看应用顺序` : '未应用规则文件'}</small>
        </span>
        ${iconTemplate(faChevronRight, 'control-icon detail-chevron')}
      </summary>
      <div class="environment-detail-list" role="list" aria-label="AGENTS.md 应用顺序">
        ${rules.order.length ? html`
          ${rules.order.map((rule) => html`
            <div class="environment-detail-item" role="listitem">
              <b class="environment-order">${rule.order}</b>
              <span><strong>${rule.label}</strong><small>${rule.scope === 'global' ? '全局基础规则' : 'Workspace 规则'}</small></span>
            </div>
          `)}
          <p class="environment-detail-note">从上到下应用，靠后的 Workspace 规则优先。</p>
        ` : html`<p class="context-empty">当前 Thread 没有实际应用 AGENTS.md。</p>`}
      </div>
    </details>
  `;
}

function serverStatus(server: SessionEnvironment['mcp']['servers'][number]) {
  if (server.status === 'connected') return `已连接 · ${server.toolCount} 个工具`;
  if (server.status === 'disabled') return '已禁用';
  return '当前不可用';
}

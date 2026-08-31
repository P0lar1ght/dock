import { html, nothing } from 'lit';
import {
  parseMarkdown,
  type MarkdownBlock,
  type MarkdownInline,
  type MarkdownListItem,
  type MarkdownTableAlignment
} from './markdown.js';
import { SAFE_EXTERNAL_LINK_REL } from './safeHtml.js';

const MARKDOWN_CACHE_LIMIT = 64;
const parsedMarkdown = new Map<string, MarkdownBlock[]>();

export function assistantMessageParts(content: string) {
  const blocks = cachedMarkdown(content);
  return html`
    <div class="message-copy markdown-message" data-testid="markdown-message">
      ${blocks.map(renderBlock)}
    </div>
  `;
}

function renderBlock(block: MarkdownBlock): unknown {
  switch (block.kind) {
    case 'heading':
      return renderHeading(block.level, block.children);
    case 'paragraph':
      return html`<p>${renderInline(block.children)}</p>`;
    case 'quote':
      return html`<blockquote>${block.children.map(renderBlock)}</blockquote>`;
    case 'list':
      return block.ordered
        ? html`<ol start=${block.start ?? nothing}>${block.items.map(renderListItem)}</ol>`
        : html`<ul>${block.items.map(renderListItem)}</ul>`;
    case 'code_block':
      return html`
        <pre class="markdown-code-block" data-testid="markdown-code-block"><code
          data-language=${block.language || nothing}
        >${block.code}</code></pre>
      `;
    case 'table':
      return renderTable(block);
    case 'thematic_break':
      return html`<hr>`;
  }
}

function cachedMarkdown(content: string) {
  const cached = parsedMarkdown.get(content);
  if (cached) {
    parsedMarkdown.delete(content);
    parsedMarkdown.set(content, cached);
    return cached;
  }
  const blocks = parseMarkdown(content);
  parsedMarkdown.set(content, blocks);
  if (parsedMarkdown.size > MARKDOWN_CACHE_LIMIT) {
    const oldest = parsedMarkdown.keys().next().value;
    if (oldest !== undefined) parsedMarkdown.delete(oldest);
  }
  return blocks;
}

function renderListItem(item: MarkdownListItem): unknown {
  const task = typeof item.checked === 'boolean';
  return html`
    <li class=${task ? 'markdown-task-item' : nothing}>
      ${task ? html`<input
        class="markdown-task-checkbox"
        type="checkbox"
        ?checked=${item.checked}
        disabled
        aria-label=${item.checked ? '已完成' : '未完成'}
      >` : nothing}
      <div class="markdown-list-content">${item.children.map(renderBlock)}</div>
    </li>
  `;
}

function renderTable(block: Extract<MarkdownBlock, { kind: 'table' }>) {
  const [head, ...body] = block.rows;
  return html`
    <div class="markdown-table-scroll" role="region" aria-label="Markdown table" tabindex="0">
      <table>
        ${head ? html`<thead><tr>${head.cells.map((cell, index) => html`
          <th data-align=${alignment(block.align[index])}>${renderInline(cell)}</th>
        `)}</tr></thead>` : nothing}
        ${body.length ? html`<tbody>${body.map((row) => html`<tr>${row.cells.map((cell, index) => html`
          <td data-align=${alignment(block.align[index])}>${renderInline(cell)}</td>
        `)}</tr>`)}</tbody>` : nothing}
      </table>
    </div>
  `;
}

function alignment(value: MarkdownTableAlignment | undefined) {
  return value || nothing;
}

function renderHeading(level: 1 | 2 | 3 | 4 | 5 | 6, children: MarkdownInline[]) {
  const content = renderInline(children);
  switch (level) {
    case 1: return html`<h1>${content}</h1>`;
    case 2: return html`<h2>${content}</h2>`;
    case 3: return html`<h3>${content}</h3>`;
    case 4: return html`<h4>${content}</h4>`;
    case 5: return html`<h5>${content}</h5>`;
    case 6: return html`<h6>${content}</h6>`;
  }
}

function renderInline(parts: MarkdownInline[]): unknown[] {
  return parts.map((part) => {
    switch (part.kind) {
      case 'text': return part.value;
      case 'break': return html`<br>`;
      case 'strong': return html`<strong>${renderInline(part.children)}</strong>`;
      case 'emphasis': return html`<em>${renderInline(part.children)}</em>`;
      case 'delete': return html`<del>${renderInline(part.children)}</del>`;
      case 'code': return html`<code class="markdown-inline-code">${part.value}</code>`;
      case 'link':
        return part.href
          ? html`<a
              href=${part.href}
              target="_blank"
              rel=${SAFE_EXTERNAL_LINK_REL}
              referrerpolicy="no-referrer"
            >${renderInline(part.children)}</a>`
          : renderInline(part.children);
    }
  });
}

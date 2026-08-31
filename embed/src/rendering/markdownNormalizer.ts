import type {
  Definition,
  ListItem,
  PhrasingContent,
  Root,
  RootContent,
  Table,
  TableCell,
  TableRow
} from 'mdast';

import { safeExternalUrl } from './safeHtml.js';
import type {
  MarkdownBlock,
  MarkdownInline,
  MarkdownListItem,
  MarkdownTableAlignment,
  MarkdownTableRow
} from './markdownModel.js';

interface NormalizationContext {
  source: string;
  definitions: Map<string, Definition>;
}

interface PositionedNode {
  position?: {
    start: { offset?: number };
    end: { offset?: number };
  };
}

export function normalizeMarkdownRoot(root: Root, source: string): MarkdownBlock[] {
  const context: NormalizationContext = {
    source,
    definitions: collectDefinitions(root)
  };
  return root.children.flatMap((node) => normalizeBlock(node, context));
}

export function normalizeInlineRoot(root: Root, source: string): MarkdownInline[] {
  const context: NormalizationContext = {
    source,
    definitions: collectDefinitions(root)
  };
  const first = root.children[0];
  if (first?.type === 'paragraph' || first?.type === 'heading') {
    return normalizeInlineChildren(first.children, context);
  }
  return source ? [{ kind: 'text', value: source }] : [];
}

function normalizeBlock(node: RootContent, context: NormalizationContext): MarkdownBlock[] {
  switch (node.type) {
    case 'paragraph':
      return [{ kind: 'paragraph', children: normalizeInlineChildren(node.children, context) }];
    case 'heading':
      return [{
        kind: 'heading',
        level: node.depth,
        children: normalizeInlineChildren(node.children, context)
      }];
    case 'blockquote':
      return [{ kind: 'quote', children: node.children.flatMap((child) => normalizeBlock(child, context)) }];
    case 'list':
      return [{
        kind: 'list',
        ordered: Boolean(node.ordered),
        ...(typeof node.start === 'number' && node.start !== 1 ? { start: node.start } : {}),
        items: node.children.map((item) => normalizeListItem(item, context))
      }];
    case 'code':
      return [{
        kind: 'code_block',
        code: node.value,
        ...(safeLanguage(node.lang) ? { language: safeLanguage(node.lang) } : {})
      }];
    case 'table':
      return [normalizeTable(node, context)];
    case 'thematicBreak':
      return [{ kind: 'thematic_break' }];
    case 'definition':
      return [];
    default:
      return fallbackBlock(node, context);
  }
}

function normalizeListItem(item: ListItem, context: NormalizationContext): MarkdownListItem {
  return {
    ...(typeof item.checked === 'boolean' ? { checked: item.checked } : {}),
    children: item.children.flatMap((child) => normalizeBlock(child, context))
  };
}

function normalizeTable(table: Table, context: NormalizationContext): MarkdownBlock {
  return {
    kind: 'table',
    align: (table.align || []).map(normalizeAlignment),
    rows: table.children.map((row) => normalizeTableRow(row, context))
  };
}

function normalizeTableRow(row: TableRow, context: NormalizationContext): MarkdownTableRow {
  return { cells: row.children.map((cell) => normalizeTableCell(cell, context)) };
}

function normalizeTableCell(cell: TableCell, context: NormalizationContext) {
  return normalizeInlineChildren(cell.children, context);
}

function normalizeAlignment(value: MarkdownTableAlignment | undefined): MarkdownTableAlignment {
  return value === 'left' || value === 'center' || value === 'right' ? value : null;
}

function normalizeInlineChildren(
  children: readonly PhrasingContent[],
  context: NormalizationContext
): MarkdownInline[] {
  const normalized: MarkdownInline[] = [];
  for (const child of children) {
    for (const part of normalizeInline(child, context)) appendInline(normalized, part);
  }
  return normalized;
}

function normalizeInline(node: PhrasingContent, context: NormalizationContext): MarkdownInline[] {
  switch (node.type) {
    case 'text':
      return textParts(node.value);
    case 'break':
      return [{ kind: 'break' }];
    case 'strong':
      return [{ kind: 'strong', children: normalizeInlineChildren(node.children, context) }];
    case 'emphasis':
      return [{ kind: 'emphasis', children: normalizeInlineChildren(node.children, context) }];
    case 'delete':
      return [{ kind: 'delete', children: normalizeInlineChildren(node.children, context) }];
    case 'inlineCode':
      return [{ kind: 'code', value: node.value }];
    case 'link':
      return [safeLink(node.url, normalizeInlineChildren(node.children, context))];
    case 'linkReference': {
      const definition = context.definitions.get(node.identifier);
      return definition
        ? [safeLink(definition.url, normalizeInlineChildren(node.children, context))]
        : fallbackInline(node, context);
    }
    default:
      return fallbackInline(node, context);
  }
}

function safeLink(url: string, children: MarkdownInline[]): MarkdownInline {
  const href = safeExternalUrl(url);
  return { kind: 'link', children, ...(href ? { href } : {}) };
}

function textParts(value: string): MarkdownInline[] {
  return value.split('\n').flatMap((part, index) => [
    ...(index ? [{ kind: 'break' as const }] : []),
    ...(part ? [{ kind: 'text' as const, value: part }] : [])
  ]);
}

function appendInline(parts: MarkdownInline[], next: MarkdownInline) {
  const previous = parts.at(-1);
  if (previous?.kind === 'text' && next.kind === 'text') previous.value += next.value;
  else parts.push(next);
}

function fallbackBlock(node: RootContent, context: NormalizationContext): MarkdownBlock[] {
  const value = sourceSlice(node, context.source);
  return value ? [{ kind: 'paragraph', children: [{ kind: 'text', value }] }] : [];
}

function fallbackInline(node: PhrasingContent, context: NormalizationContext): MarkdownInline[] {
  const value = sourceSlice(node, context.source);
  return value ? [{ kind: 'text', value }] : [];
}

function sourceSlice(node: PositionedNode, source: string) {
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  return Number.isInteger(start) && Number.isInteger(end) && start! >= 0 && end! >= start! && end! <= source.length
    ? source.slice(start, end)
    : '';
}

function collectDefinitions(root: Root) {
  const definitions = new Map<string, Definition>();
  const visit = (node: unknown) => {
    if (!node || typeof node !== 'object') return;
    const current = node as { type?: string; identifier?: string; children?: unknown[] };
    if (current.type === 'definition' && current.identifier && !definitions.has(current.identifier)) {
      definitions.set(current.identifier, node as Definition);
    }
    current.children?.forEach(visit);
  };
  visit(root);
  return definitions;
}

function safeLanguage(value: string | null | undefined) {
  const language = String(value || '').trim().split(/\s+/u)[0] || '';
  return /^[A-Za-z0-9_+.-]{1,32}$/u.test(language) ? language : undefined;
}

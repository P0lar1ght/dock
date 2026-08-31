export type MarkdownInline =
  | { kind: 'text'; value: string }
  | { kind: 'break' }
  | { kind: 'strong'; children: MarkdownInline[] }
  | { kind: 'emphasis'; children: MarkdownInline[] }
  | { kind: 'delete'; children: MarkdownInline[] }
  | { kind: 'code'; value: string }
  | { kind: 'link'; children: MarkdownInline[]; href?: string };

export interface MarkdownListItem {
  checked?: boolean;
  children: MarkdownBlock[];
}

export type MarkdownTableAlignment = 'left' | 'center' | 'right' | null;

export interface MarkdownTableRow {
  cells: MarkdownInline[][];
}

export type MarkdownBlock =
  | { kind: 'paragraph'; children: MarkdownInline[] }
  | { kind: 'heading'; level: 1 | 2 | 3 | 4 | 5 | 6; children: MarkdownInline[] }
  | { kind: 'quote'; children: MarkdownBlock[] }
  | { kind: 'list'; ordered: boolean; start?: number; items: MarkdownListItem[] }
  | { kind: 'code_block'; code: string; language?: string }
  | { kind: 'table'; align: MarkdownTableAlignment[]; rows: MarkdownTableRow[] }
  | { kind: 'thematic_break' };

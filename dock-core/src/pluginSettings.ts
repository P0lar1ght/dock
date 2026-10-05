// 插件设置卡 `plugin/settings/*`（`docs/PLUGIN-VIEWS.md` 设置卡）。schema 外形与校验文案
// 和 `cordis-base/src/plugin_settings.rs` 一一对应：界面先按它标错、禁用保存，
// 最终以 Dock 回的 `errors` 为准。

export type SettingsFieldType = 'string' | 'text' | 'number' | 'boolean' | 'select' | 'secret';

export interface SettingsField {
  key: string;
  type: SettingsFieldType;
  label: string;
  /** 字段下面的灰字说明。 */
  description: string | null;
  default: unknown;
  required: boolean;
  /** `number` 才有。 */
  min: number | null;
  max: number | null;
  /** `select` 才有。 */
  options: string[];
}

export interface SettingsSchema {
  title: string;
  fields: SettingsField[];
}

/** `plugin/settings/get`（或 `set` 成功回的 `settings`）。 */
export interface PluginSettingsSnapshot {
  pluginId: string;
  schema: SettingsSchema;
  /** 普通字段的当前值（没写过是 default，没有 default 是 `null`）。 */
  values: Record<string, unknown>;
  /** 密钥字段设没设；界面永远拿不到原值。 */
  secrets: Record<string, boolean>;
}

/** `plugin/settings/set` 的结果：失败时逐字段的原因，整组没写。 */
export type PluginSettingsSetResult =
  | { ok: true; settings: PluginSettingsSnapshot | null }
  | { ok: false; errors: Record<string, string> };

type Obj = Record<string, unknown>;

const TYPES: readonly SettingsFieldType[] = ['string', 'text', 'number', 'boolean', 'select', 'secret'];

function obj(v: unknown): Obj {
  return v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Obj) : {};
}

function numOrNull(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

/** 网关给的 schema 已经规范化过；认不出 type 的字段丢掉，界面不画它。 */
export function parseSettingsSchema(v: unknown): SettingsSchema {
  const s = obj(v);
  const fields = Array.isArray(s.fields) ? s.fields : [];
  return {
    title: typeof s.title === 'string' ? s.title : '',
    fields: fields
      .map(obj)
      .filter((f) => typeof f.key === 'string' && f.key && TYPES.includes(f.type as SettingsFieldType))
      .map((f) => ({
        key: f.key as string,
        type: f.type as SettingsFieldType,
        label: typeof f.label === 'string' && f.label ? f.label : (f.key as string),
        description: typeof f.description === 'string' && f.description ? f.description : null,
        default: f.default ?? null,
        required: f.required === true,
        min: numOrNull(f.min),
        max: numOrNull(f.max),
        options: Array.isArray(f.options) ? f.options.filter((o): o is string => typeof o === 'string') : [],
      })),
  };
}

export function parsePluginSettingsList(result: unknown): { pluginId: string; title: string }[] {
  const list = obj(result).plugins;
  if (!Array.isArray(list)) return [];
  return list
    .map(obj)
    .filter((p) => typeof p.pluginId === 'string' && p.pluginId)
    .map((p) => ({ pluginId: p.pluginId as string, title: typeof p.title === 'string' ? p.title : (p.pluginId as string) }));
}

export function parsePluginSettings(value: unknown): PluginSettingsSnapshot | null {
  const s = obj(value);
  if (typeof s.pluginId !== 'string' || !s.pluginId) return null;
  const secrets: Record<string, boolean> = {};
  for (const [k, v] of Object.entries(obj(s.secrets))) secrets[k] = v === true;
  return { pluginId: s.pluginId, schema: parseSettingsSchema(s.schema), values: { ...obj(s.values) }, secrets };
}

export function parsePluginSettingsSet(result: unknown): PluginSettingsSetResult {
  const r = obj(result);
  if (r.ok === true) return { ok: true, settings: parsePluginSettings(r.settings) };
  const errors: Record<string, string> = {};
  for (const [k, v] of Object.entries(obj(r.errors))) errors[k] = typeof v === 'string' ? v : String(v);
  return { ok: false, errors };
}

/** `plugin/settings/changed { pluginId }`（连接级）。认不出回 `null`。 */
export function parsePluginSettingsPush(method: string, params: unknown): { pluginId: string } | null {
  if (method !== 'plugin/settings/changed') return null;
  const id = obj(params).pluginId;
  return typeof id === 'string' ? { pluginId: id } : null;
}

/** 一个值合不合这个字段：合就回规范化后的值（数字串转成数），否则回原因（同 Dock 的文案）。 */
export function checkSettingsField(field: SettingsField, value: unknown): { ok: true; value: unknown } | { ok: false; error: string } {
  const fail = (error: string) => ({ ok: false as const, error });
  switch (field.type) {
    case 'string':
    case 'text':
    case 'secret': {
      if (typeof value !== 'string') return fail('要一段文字');
      if (field.required && value.trim() === '') return fail('必填');
      if (field.type === 'string' && value.includes('\n')) return fail('只能一行');
      return { ok: true, value };
    }
    case 'number': {
      let n: number | null = null;
      if (typeof value === 'number') n = value;
      else if (typeof value === 'string' && value.trim() !== '') n = Number(value.trim());
      if (n === null || Number.isNaN(n)) return fail('要一个数');
      if (field.min !== null && n < field.min) return fail(`最小 ${field.min}`);
      if (field.max !== null && n > field.max) return fail(`最大 ${field.max}`);
      return { ok: true, value: n };
    }
    case 'boolean':
      return typeof value === 'boolean' ? { ok: true, value } : fail('要开或关');
    case 'select':
      if (typeof value !== 'string') return fail('要选一项');
      return field.options.includes(value) ? { ok: true, value } : fail(`只能是 ${field.options.join(' / ')}`);
  }
}

/**
 * 表单里要改的那些值逐个校验（同 Dock 的 `validate`）。`null` = 清回默认，必填且没有 default 不能清；
 * 密钥字段空串 = 不改，不校验。回 `{ key: 原因 }`，空对象 = 可以保存。
 */
export function settingsErrors(schema: SettingsSchema, values: Record<string, unknown>): Record<string, string> {
  const errors: Record<string, string> = {};
  for (const [key, value] of Object.entries(values)) {
    const field = schema.fields.find((f) => f.key === key);
    if (!field) {
      errors[key] = '没有这个字段';
      continue;
    }
    if (field.type === 'secret' && value === '') continue;
    if (value === null) {
      if (field.required && field.default === null) errors[key] = '必填';
      continue;
    }
    const r = checkSettingsField(field, value);
    if (!r.ok) errors[key] = r.error;
  }
  return errors;
}

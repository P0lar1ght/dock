// 插件设置卡：设置 › 插件与技能 里登记了 `plugin.settings` 的插件，行右侧多一个「配置」，
// 点开在这一行下面就地展开表单。设计：wonder-ui/plugins/SettingsPluginsConfig（sir）、
// …Invalid（but）、…Saved（say）。
// - 表单按 schema 生成：select / secret / boolean / number / string / text，字段下灰字 description。
// - 校验和 Dock 同一套（dock-core `settingsErrors`）：有错标红、保存禁用；最终以 Dock 回的为准。
// - 密钥只进不出：已设置时显示「已设置 · 留空不改」，留空就不改。
// - 保存后按钮旁「已保存」1.5 秒淡出。

import { useEffect, useState, useSyncExternalStore } from "react";
import { Check, ChevronDown, LoaderCircle } from "lucide-react";
import { settingsErrors, type PluginSettingsSnapshot, type SettingsField } from "dock-core";
import { Select, TextInput, Toggle } from "../../components/settings/ui";
import type { PluginRowProps } from "../registry";
import { onPluginSettingsChanged, pluginSettingsGet, pluginSettingsList, pluginSettingsSet } from "./api";

// 有设置卡的插件 id：全局一份，`plugin/settings/changed` 时重拉。
let ids = new Set<string>();
const listeners = new Set<() => void>();
let started = false;

function start() {
  if (started) return;
  started = true;
  const load = () =>
    pluginSettingsList().then(
      (list) => {
        ids = new Set(list.map((p) => p.pluginId));
        listeners.forEach((fn) => fn());
      },
      () => {},
    );
  void load();
  onPluginSettingsChanged(() => void load());
}

function useHasSettings(pluginId: string) {
  const set = useSyncExternalStore(
    (fn) => {
      start();
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => ids,
  );
  return set.has(pluginId);
}

export function PluginSettingsRow({ pluginId, children }: PluginRowProps) {
  const has = useHasSettings(pluginId);
  const [open, setOpen] = useState(false);
  const [seen, setSeen] = useState(false);
  if (!has) return <>{children({ action: null, panel: null })}</>;
  const action = (
    <button
      type="button"
      aria-expanded={open}
      onClick={() => {
        setSeen(true);
        setOpen((o) => !o);
      }}
      className="motion-colors flex items-center gap-0.5 text-[11px] text-accent hover:opacity-80"
    >
      配置
      <ChevronDown size={12} className={`transition-transform duration-[var(--dur-base)] ${open ? "rotate-180" : ""}`} />
    </button>
  );
  // 展开 / 收起：grid 行高 + 透明度（MOTION.md）。
  const panel = (
    <div
      className="grid transition-[grid-template-rows,opacity] duration-[var(--dur-base)] ease-[var(--ease-out)]"
      style={{ gridTemplateRows: open ? "1fr" : "0fr", opacity: open ? 1 : 0 }}
    >
      <div className="min-h-0 overflow-hidden">{seen && <SettingsForm pluginId={pluginId} active={open} />}</div>
    </div>
  );
  return <>{children({ action, panel })}</>;
}

type Draft = Record<string, string | boolean>;

/** 快照里的值 → 表单里的初值。密钥一律空（留空不改）。 */
function draftOf(snap: PluginSettingsSnapshot): Draft {
  const out: Draft = {};
  for (const f of snap.schema.fields) {
    const v = snap.values[f.key];
    out[f.key] = f.type === "boolean" ? v === true : f.type === "secret" || v == null ? "" : String(v);
  }
  return out;
}

/** 表单值 → 要写的值：数字 / 下拉留空 = 清回默认（null）。 */
function wireOf(field: SettingsField, v: string | boolean): unknown {
  if ((field.type === "number" || field.type === "select") && v === "") return null;
  return v;
}

function SettingsForm({ pluginId, active }: { pluginId: string; active: boolean }) {
  const [snap, setSnap] = useState<PluginSettingsSnapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft>({});
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  // 每次展开都重拉一份（别处可能改过）。
  useEffect(() => {
    if (!active) return;
    let alive = true;
    pluginSettingsGet(pluginId).then(
      (s) => {
        if (!alive) return;
        setSnap(s);
        if (s) setDraft(draftOf(s));
        setLoadError(s ? null : "这个插件没有设置卡了");
      },
      (e) => alive && setLoadError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      alive = false;
    };
  }, [pluginId, active]);

  useEffect(() => {
    if (savedAt === null) return;
    const t = setTimeout(() => setSavedAt(null), 1500);
    return () => clearTimeout(t);
  }, [savedAt]);

  if (loadError) return <div className="border-t border-line-soft bg-pane px-4 py-3 text-xs text-[#d70015]">{loadError}</div>;
  if (!snap) return <div className="border-t border-line-soft bg-pane px-4 py-3 text-xs text-ink-4">正在读取配置…</div>;

  const initial = draftOf(snap);
  const changed: Record<string, unknown> = {};
  for (const f of snap.schema.fields) {
    const v = draft[f.key];
    if (f.type === "secret" ? v !== "" : v !== initial[f.key]) changed[f.key] = wireOf(f, v);
  }
  const errors = { ...settingsErrors(snap.schema, changed), ...serverErrors };
  const canSave = Object.keys(changed).length > 0 && Object.keys(errors).length === 0 && !saving;

  const set = (key: string, v: string | boolean) => {
    setDraft((d) => ({ ...d, [key]: v }));
    setServerErrors(({ [key]: _, ...rest }) => rest);
  };

  const save = async () => {
    setSaving(true);
    try {
      const r = await pluginSettingsSet(pluginId, changed);
      if (r.ok) {
        if (r.settings) {
          setSnap(r.settings);
          setDraft(draftOf(r.settings));
        }
        setServerErrors({});
        setSavedAt(Date.now());
      } else {
        setServerErrors(r.errors);
      }
    } catch (e) {
      setServerErrors({ "": e instanceof Error ? e.message : String(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-4 border-t border-line-soft bg-pane p-4">
      {snap.schema.fields.map((f) => (
        <Field key={f.key} field={f} value={draft[f.key]} secretSet={snap.secrets[f.key]} error={errors[f.key]} onChange={(v) => set(f.key, v)} />
      ))}
      {errors[""] && <span className="text-[11px] font-medium text-[#d70015]">{errors[""]}</span>}
      <div className="flex items-center gap-2 pt-1">
        <button
          type="button"
          disabled={!canSave}
          onClick={() => void save()}
          className="motion-colors flex h-8 items-center gap-1.5 rounded-lg bg-ink px-3 text-xs font-medium text-white hover:bg-ink-2 disabled:bg-[#f2f2f7] disabled:text-ink-5"
        >
          {saving && <LoaderCircle size={12} className="animate-spin" />}
          保存
        </button>
        {savedAt !== null && (
          <span key={savedAt} className="enter-fade flex items-center gap-1 text-xs font-medium text-[#248a3d]">
            <Check size={12} />
            已保存
          </span>
        )}
      </div>
    </div>
  );
}

function Field({
  field,
  value,
  secretSet,
  error,
  onChange,
}: {
  field: SettingsField;
  value: string | boolean | undefined;
  secretSet: boolean | undefined;
  error: string | undefined;
  onChange: (v: string | boolean) => void;
}) {
  const label = <span className="text-[13px] font-medium leading-none text-ink-2">{field.label}</span>;
  const desc = field.description && <span className="text-[11px] leading-normal text-ink-4">{field.description}</span>;
  const err = error && <span className="enter-fade text-[11px] font-medium leading-normal text-[#d70015]">{error}</span>;
  if (field.type === "boolean") {
    return (
      <div className="flex flex-col gap-1.5">
        <div className="flex w-full max-w-[360px] items-center justify-between">
          {label}
          <Toggle on={value === true} onChange={onChange} label={field.label} />
        </div>
        {err}
        {desc}
      </div>
    );
  }
  const text = typeof value === "string" ? value : "";
  let control;
  if (field.type === "select") {
    control = (
      <Select
        value={text}
        options={field.options.map((o) => ({ value: o, label: o }))}
        onChange={onChange}
        width={360}
        placeholder={field.required ? undefined : "（默认）"}
      />
    );
  } else if (field.type === "text") {
    control = (
      <textarea
        value={text}
        rows={3}
        placeholder={field.required ? "" : "可选"}
        onChange={(e) => onChange(e.target.value)}
        className={`motion-colors thin-scroll h-[72px] w-full max-w-[360px] resize-none rounded-lg border bg-white px-3 py-2 text-[13px] text-ink shadow-sm outline-none placeholder:text-ink-4 focus:border-accent ${
          error ? "border-[#d70015] shadow-[0_0_0_3px_rgba(215,0,21,0.12)]" : "border-line-strong"
        }`}
      />
    );
  } else {
    control = (
      <div className={`max-w-[360px] rounded-lg ${error ? "shadow-[0_0_0_3px_rgba(215,0,21,0.12)]" : ""}`}>
        <TextInput
          value={text}
          onChange={onChange}
          width="100%"
          type={field.type === "secret" ? "password" : field.type === "number" ? "number" : "text"}
          invalid={!!error}
          placeholder={field.type === "secret" ? (secretSet ? "已设置 · 留空不改" : "未设置") : field.required ? "" : "可选"}
        />
      </div>
    );
  }
  return (
    <label className="flex flex-col gap-1.5">
      {label}
      {control}
      {err}
      {desc}
    </label>
  );
}

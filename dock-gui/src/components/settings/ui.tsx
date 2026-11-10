// 设置页的小件：页头、分节卡、设置行、开关、分段、下拉、输入框、胶囊、横条、页签、弹窗。
// 取值照 wonder-ui/settings（goy / uta / log 等），所有分组共用这一套。

import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { Check, ChevronDown, LoaderCircle } from "lucide-react";
import { imeComposing } from "../../dock/ime";

// ---- 保存状态：右上角「已保存」+ 页顶「写入失败」红条 ----

let saveState: { savedAt: number | null; error: string | null } = { savedAt: null, error: null };
const saveListeners = new Set<() => void>();
const emitSave = (next: typeof saveState) => {
  saveState = next;
  for (const l of saveListeners) l();
};

/** 即改即存的控件写成功后调：页头闪一下「已保存」，清掉写入失败的红条。 */
export const flashSaved = () => emitSave({ savedAt: Date.now(), error: null });
/** 写盘失败：页顶红条，直到下一次写成功。 */
export const reportWriteError = (message: string) => emitSave({ savedAt: null, error: message });
export const clearSaveState = () => emitSave({ savedAt: null, error: null });

function useSaveState() {
  return useSyncExternalStore(
    (l) => {
      saveListeners.add(l);
      return () => saveListeners.delete(l);
    },
    () => saveState,
  );
}

/** 跑一次即改即存：成功闪「已保存」，失败挂红条。回 Promise 的结果（失败回 null）。 */
export async function autosave<T>(write: () => Promise<T>, onError?: (message: string) => void): Promise<T | null> {
  try {
    const r = await write();
    flashSaved();
    return r;
  } catch (e) {
    const message = e instanceof Error ? e.message.replace(/^[\w/]+: /, "") : String(e);
    reportWriteError(message);
    onError?.(message);
    return null;
  }
}

// ---- 页面骨架 ----

export function Page({
  title,
  subtitle,
  back,
  actions,
  banner,
  children,
}: {
  title: string;
  subtitle?: ReactNode;
  back?: { label: string; onClick: () => void };
  actions?: ReactNode;
  banner?: ReactNode;
  children: ReactNode;
}) {
  const { savedAt, error } = useSaveState();
  const [showSaved, setShowSaved] = useState(false);
  // 只认这一页打开之后的保存：上一页留下的「已保存」不跟到新页上来。
  const [openedAt] = useState(() => Date.now());
  useEffect(() => {
    if (!savedAt || savedAt < openedAt) return;
    setShowSaved(true);
    const t = window.setTimeout(() => setShowSaved(false), 1600);
    return () => window.clearTimeout(t);
  }, [savedAt, openedAt]);
  return (
    <div className="flex w-full max-w-[720px] flex-col gap-5">
      {error && <Banner kind="error">{error}</Banner>}
      {banner}
      <div className="flex shrink-0 items-start justify-between gap-4 border-b border-line-soft pb-4">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          {back && (
            <button
              type="button"
              onClick={back.onClick}
              className="motion-colors mb-1 self-start text-[12.5px] font-medium text-accent hover:underline"
            >
              ← {back.label}
            </button>
          )}
          <h1 className="text-[22px] font-semibold leading-tight tracking-[-0.3px] text-ink">{title}</h1>
          {subtitle && <p className="text-[12.5px] text-ink-4">{subtitle}</p>}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {showSaved && (
            <span className="enter-fade flex items-center gap-1.5 rounded-md bg-[#34c7591a] px-2 py-1 text-[11px] font-medium text-[#248a3d]">
              <Check size={12} />
              已保存
            </span>
          )}
          {actions}
        </div>
      </div>
      {children}
    </div>
  );
}

export function Section({ title, right, children }: { title: string; right?: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-[11px] font-semibold tracking-[0.3px] text-ink-4">{title}</h2>
        {right}
      </div>
      {children}
    </section>
  );
}

export const cardClass = "md-surface flex min-w-0 flex-col overflow-hidden rounded-xl border border-line-soft bg-white shadow-sm";

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`${cardClass} ${className}`}>{children}</div>;
}

/** 一行设置：左名字 + 说明，右控件。行间细分隔线（最后一行没有）。 */
export function Row({
  label,
  desc,
  keyHint,
  children,
  top,
  disabled,
  className = "",
}: {
  label: ReactNode;
  desc?: ReactNode;
  /** 等宽灰字的配置键名，如 `api_base_url`。 */
  keyHint?: string;
  children?: ReactNode;
  top?: boolean;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <div
      className={`flex justify-between gap-4 border-b border-[#f0f0f3] px-4 py-3 last:border-b-0 ${top ? "items-start" : "items-center"} ${disabled ? "opacity-50" : ""} ${className}`}
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-[13px] font-medium text-ink">{label}</span>
        {keyHint && <span className="font-mono text-[11px] text-ink-4">{keyHint}</span>}
        {desc && <span className="text-xs text-ink-4">{desc}</span>}
      </div>
      {children !== undefined && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  );
}

// ---- 控件 ----

export function Toggle({
  on,
  onChange,
  disabled,
  label,
}: {
  on: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!on)}
      className={`flex h-5 w-9 shrink-0 items-center rounded-full p-0.5 transition-colors duration-200 disabled:cursor-not-allowed disabled:opacity-50 ${on ? "bg-accent" : "bg-line-strong"}`}
    >
      <span
        className={`glass-keep size-4 rounded-full bg-white shadow transition-transform duration-200 ease-out ${on ? "translate-x-4" : ""}`}
      />
    </button>
  );
}

/** 滑块：拖动时就回调（改了立即生效），右侧显示当前值。 */
export function Range({
  value,
  onChange,
  min = 0,
  max = 100,
  step = 1,
  format = (v) => `${v}%`,
  label,
  width = 180,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  format?: (v: number) => string;
  label?: string;
  width?: number;
}) {
  return (
    <div className="flex items-center gap-2.5">
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-label={label}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1 cursor-pointer accent-[var(--color-accent)]"
        style={{ width }}
      />
      <span className="w-9 shrink-0 text-right font-mono text-[11.5px] text-ink-3 tabular-nums">{format(value)}</span>
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  disabled,
}: {
  value: T;
  options: { value: T; label: string; disabled?: boolean; title?: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  return (
    <div className={`flex items-center gap-0.5 rounded-lg bg-[#eaeaee] p-0.5 ${disabled ? "opacity-50" : ""}`}>
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            title={o.title}
            disabled={disabled || o.disabled}
            onClick={() => onChange(o.value)}
            className={`motion-colors flex h-7 items-center rounded-md px-2.5 text-xs disabled:cursor-not-allowed ${
              on ? "bg-white font-semibold text-ink shadow-sm" : "text-ink-3 hover:text-ink disabled:hover:text-ink-3"
            } ${o.disabled ? "opacity-50" : ""}`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export function Select({
  value,
  options,
  onChange,
  width = 140,
  disabled,
  placeholder,
}: {
  value: string;
  options: { value: string; label: string }[];
  onChange: (v: string) => void;
  width?: number;
  disabled?: boolean;
  placeholder?: string;
}) {
  return (
    <label
      className={`motion-colors relative flex h-8 items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 shadow-sm focus-within:border-accent ${disabled ? "opacity-50" : "hover:border-ink-5"}`}
      style={{ width }}
    >
      <span className={`min-w-0 flex-1 truncate text-[12.5px] ${value ? "text-ink" : "text-ink-4"}`}>
        {options.find((o) => o.value === value)?.label ?? (value || placeholder || "")}
      </span>
      <ChevronDown size={13} className="shrink-0 text-ink-4" />
      <select
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className="absolute inset-0 cursor-pointer opacity-0 disabled:cursor-not-allowed"
      >
        {placeholder !== undefined && <option value="">{placeholder}</option>}
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

export function TextInput({
  value,
  onChange,
  onCommit,
  placeholder,
  width = 280,
  mono,
  type = "text",
  disabled,
  invalid,
  suffix,
  autoFocus,
}: {
  value: string;
  onChange?: (v: string) => void;
  /** 失焦或回车时提交（即改即存的输入框用）。 */
  onCommit?: (v: string) => void;
  placeholder?: string;
  width?: number | string;
  mono?: boolean;
  type?: "text" | "password" | "number";
  disabled?: boolean;
  invalid?: boolean;
  suffix?: ReactNode;
  autoFocus?: boolean;
}) {
  // 即改即存：本地先跟着打字，失焦 / 回车才提交，外面的值变了再同步回来。
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (onCommit && draft !== value) onCommit(draft);
  };
  return (
    <div
      className={`motion-colors flex h-8 items-center gap-1.5 rounded-lg border bg-white px-2.5 shadow-sm focus-within:border-accent ${
        invalid ? "border-[#ff3b30]" : "border-line-strong"
      } ${disabled ? "bg-window shadow-none" : ""}`}
      style={{ width }}
    >
      <input
        type={type}
        value={onCommit ? draft : value}
        disabled={disabled}
        autoFocus={autoFocus}
        placeholder={placeholder}
        spellCheck={false}
        onChange={(e) => (onCommit ? setDraft(e.target.value) : onChange?.(e.target.value))}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !imeComposing(e)) (e.target as HTMLInputElement).blur();
        }}
        className={`min-w-0 flex-1 bg-transparent text-ink outline-none placeholder:text-ink-4 disabled:text-ink-4 ${
          mono ? "font-mono text-xs" : "text-[12.5px]"
        } [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none`}
      />
      {suffix && <span className="shrink-0 text-[11.5px] text-ink-4">{suffix}</span>}
    </div>
  );
}

export function Checkbox({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`motion-colors flex size-3.5 shrink-0 items-center justify-center rounded-sm border ${
        checked ? "border-accent bg-accent" : "border-ink-5 bg-white"
      }`}
    >
      {checked && <Check size={10} strokeWidth={3} className="text-white" />}
    </button>
  );
}

type PillKind = "gray" | "green" | "orange" | "red" | "blue";
const PILL: Record<PillKind, string> = {
  gray: "bg-[#f2f2f7] text-ink-3",
  green: "bg-[#34c7591a] text-[#248a3d]",
  orange: "bg-[#ff9f0a1a] text-[#c93400]",
  red: "bg-[#ff3b301a] text-[#d70015]",
  blue: "bg-[#0a84ff1a] text-accent",
};

export function Pill({ kind = "gray", children, title }: { kind?: PillKind; children: ReactNode; title?: string }) {
  return (
    <span title={title} className={`inline-flex shrink-0 items-center rounded-full px-2 py-0.5 text-[11px] font-medium ${PILL[kind]}`}>
      {children}
    </span>
  );
}

/** 「环境变量覆盖」小胶囊：悬停说是哪个变量。 */
export function EnvPill({ name }: { name: string }) {
  return (
    <span title={`被环境变量 ${name} 覆盖`} className="rounded-full bg-[#f2f2f7] px-1.5 py-0.5 text-[10px] text-ink-4">
      环境变量覆盖
    </span>
  );
}

export const primaryButton =
  "motion-colors flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-ink px-3 text-[12.5px] font-medium text-white shadow-sm hover:bg-ink-2 disabled:opacity-50";
export const secondaryButton =
  "motion-colors flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-line-strong bg-white px-3 text-[12.5px] font-medium text-ink shadow-sm hover:bg-hover disabled:opacity-50";
export const smallButton =
  "motion-colors flex h-7 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-2 text-[11.5px] font-medium text-ink shadow-sm hover:bg-hover disabled:opacity-50";
export const dangerText = "motion-colors rounded-md px-2 py-1 text-[12px] font-medium text-[#ff3b30] hover:bg-[#ff3b3014]";

export function Banner({
  kind,
  children,
  action,
}: {
  kind: "error" | "info" | "warn";
  children: ReactNode;
  action?: ReactNode;
}) {
  const cls = {
    error: "border-[#ff3b3033] bg-[#ff3b3014] text-[#d70015]",
    info: "border-[#0a84ff33] bg-[#0a84ff14] text-[#0040dd]",
    warn: "border-[#ff9f0a4d] bg-[#fff8e6] text-[#8a5a00]",
  }[kind];
  return (
    <div className={`enter-fade flex items-center gap-2 rounded-lg border px-3 py-2.5 text-[12.5px] ${cls}`}>
      <span className="min-w-0 flex-1 break-words">{children}</span>
      {action}
    </div>
  );
}

export function Tabs<T extends string>({
  value,
  tabs,
  onChange,
}: {
  value: T;
  tabs: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="flex border-b border-line-soft">
      {tabs.map((t) => (
        <button
          key={t.value}
          type="button"
          onClick={() => onChange(t.value)}
          className={`motion-colors -mb-px border-b-2 px-3 py-2 text-[13px] ${
            t.value === value ? "border-accent font-medium text-accent" : "border-transparent text-ink-4 hover:text-ink-2"
          }`}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

/** 盖住整个窗口（含左栏）的弹窗。 */
export function Modal({ children, onClose, width = 420 }: { children: ReactNode; onClose?: () => void; width?: number }) {
  useEffect(() => {
    if (!onClose) return;
    // 捕获阶段拦 Esc：不然先关掉了整个设置页。
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);
  return (
    <div className="enter-fade fixed inset-0 z-[60] flex items-center justify-center bg-black/40" onMouseDown={onClose}>
      <div
        className="glass-dialog pop-in flex max-w-[calc(100vw-48px)] flex-col gap-3 rounded-2xl border border-line-soft bg-white p-5 shadow-xl"
        style={{ width }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}

export function Loading({ label = "正在读取…" }: { label?: string }) {
  return (
    <div className="flex items-center gap-2 text-[12.5px] text-ink-4">
      <LoaderCircle size={14} className="animate-spin" />
      {label}
    </div>
  );
}

export function ErrorLine({ children }: { children: ReactNode }) {
  return <div className="enter-fade text-[12.5px] text-[#c0392b]">{children}</div>;
}

/** 读一份数据：loading / error / data，外加手动刷新和就地替换。 */
export function useLoad<T>(load: () => Promise<T>, deps: unknown[] = []) {
  const [state, setState] = useState<{ data?: T; error?: string; loading: boolean }>({ loading: true });
  const reload = () => {
    setState((s) => ({ ...s, loading: true }));
    return load()
      .then((data) => setState({ data, loading: false }))
      .catch((e) => setState((s) => ({ ...s, error: e instanceof Error ? e.message.replace(/^[\w/]+: /, "") : String(e), loading: false })));
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => void reload(), deps);
  return { ...state, reload, set: (data: T) => setState({ data, loading: false }) };
}

/** 主机名：`https://api.x.com/v1` → `api.x.com`。 */
export function host(url: string | null): string {
  if (!url) return "未配置地址";
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

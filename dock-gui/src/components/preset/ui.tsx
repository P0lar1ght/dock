// 预设编辑器共用的小件：开关、分段选择、字段、分区标题（输入框样式在 util.ts）。样式照 wonder-ui/preset/PresetEditor*。

import type { ReactNode } from "react";

export function Switch({
  on,
  onChange,
  size = "md",
  color = "accent",
  label,
}: {
  on: boolean;
  onChange: (on: boolean) => void;
  size?: "sm" | "md";
  color?: "accent" | "green";
  label: string;
}) {
  const box = size === "sm" ? "h-[18px] w-[34px]" : "h-[22px] w-10";
  const knob = size === "sm" ? "size-3.5" : "size-[18px]";
  const shift = size === "sm" ? "translate-x-4" : "translate-x-[18px]";
  const bg = on ? (color === "green" ? "bg-[#34c759]" : "bg-accent") : "bg-line-strong";
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={`flex shrink-0 items-center rounded-full p-0.5 transition-colors duration-[var(--dur-base)] ${box} ${bg}`}
    >
      <span
        className={`block rounded-full bg-white shadow transition-transform duration-[var(--dur-base)] ease-[var(--ease-out)] ${knob} ${
          on ? shift : ""
        }`}
      />
    </button>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = "md",
}: {
  value: T;
  options: { id: T; label: ReactNode; disabled?: boolean; title?: string }[];
  onChange: (v: T) => void;
  size?: "sm" | "md";
}) {
  return (
    <div className={`flex items-center gap-0.5 rounded-lg bg-line-strong p-0.5 ${size === "md" ? "h-8" : ""}`}>
      {options.map((o) => {
        const on = o.id === value;
        return (
          <button
            key={o.id}
            type="button"
            disabled={o.disabled}
            title={o.title}
            onClick={() => onChange(o.id)}
            className={`motion-colors flex-1 whitespace-nowrap rounded-md px-3 py-1 text-xs disabled:cursor-default disabled:opacity-50 ${
              on ? "bg-white font-semibold text-accent shadow-sm" : "font-medium text-ink-3 enabled:hover:text-ink"
            }`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export function Field({
  label,
  hint,
  error,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  error?: string | null;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-3">
        <span className={`text-[13px] font-semibold ${error ? "text-[#ff3b30]" : "text-ink"}`}>{label}</span>
        {hint && <span className="font-mono text-[11.5px] text-ink-4">{hint}</span>}
      </div>
      {children}
      {error && <span className="enter-fade text-[11.5px] text-[#ff3b30]">⊗ {error}</span>}
    </div>
  );
}

/** 分区标题（设计：20px 标题 + 灰色副标题 + 底线）。 */
export function SectionHeader({ title, subtitle, actions }: { title: string; subtitle: string; actions?: ReactNode }) {
  return (
    <div className="flex shrink-0 items-start justify-between gap-4 border-b border-line-soft pb-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-xl font-semibold leading-tight text-ink">{title}</h2>
        <p className="text-[13px] text-ink-4">{subtitle}</p>
      </div>
      {actions}
    </div>
  );
}

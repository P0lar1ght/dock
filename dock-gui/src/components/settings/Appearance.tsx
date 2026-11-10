// 外观（zad）：主题、强调色、材质、界面字号、字体与代码、对话显示、侧栏。GUI 自己的偏好，改了立即生效。
// 第一版只做浅色：深色 / 跟随系统保留外观，标「即将推出」。
// 字体只换对话正文（回复 + 你发的消息）和等宽字；界面外壳固定用系统字体。

import { Check } from "lucide-react";
import { CHAT_FONTS, CODE_FONTS, prefs, usePrefs, type ChatFont, type CodeFont, type Prefs } from "../../dock/prefs";
import { COMING_SOON } from "../chrome";
import { Markdown } from "../Markdown";
import { CODE_THEMES, preloadTheme } from "../markdown/highlight";
import { Card, Page, Range, Row, Section, Segmented, Select, Toggle, flashSaved } from "./ui";

const SAMPLE = [
  "把 `SessionStore` 改成**懒加载**：首屏只读最近 50 条，复杂度从 $O(n)$ 降到 $O(1)$。",
  "```rust\n// 按游标分页\npub fn load_page(&self, cursor: Option<u64>) -> Result<Page> {\n    let rows = self.db.query(\"SELECT * FROM msgs\", cursor)?;\n    Ok(Page { rows, next: None })\n}\n```",
].join("\n\n");

const ACCENTS = ["#0a84ff", "#34c759", "#ff9f0a", "#ff3b30", "#af52de", "#64d2ff"];

export function AppearancePane() {
  const p = usePrefs();
  const set = <K extends keyof Prefs>(k: K, v: Prefs[K]) => {
    prefs.set(k, v);
    flashSaved();
  };
  return (
    <Page title="外观" subtitle="主题、强调色与对话显示偏好">
      <Section title="主题与强调色">
        <Card>
          <div className="flex gap-3 border-b border-[#f0f0f3] px-4 py-3">
            <ThemeCard label="浅色" selected>
              <div className="h-full rounded-[6px] bg-[#f5f5f7]" />
            </ThemeCard>
            <ThemeCard label="深色" soon>
              <div className="h-full rounded-[6px] bg-[#1d1d1f]" />
            </ThemeCard>
            <ThemeCard label="跟随系统" soon>
              <div className="flex h-full overflow-hidden rounded-[6px]">
                <div className="flex-1 bg-[#f5f5f7]" />
                <div className="flex-1 bg-[#1d1d1f]" />
              </div>
            </ThemeCard>
          </div>
          <Row label={`强调色 ${p.accent}`}>
            <div className="flex gap-2">
              {ACCENTS.map((c) => (
                <button
                  key={c}
                  type="button"
                  aria-label={c}
                  onClick={() => set("accent", c)}
                  className={`flex size-[18px] items-center justify-center rounded-full transition-transform hover:scale-110 ${
                    p.accent === c ? "ring-2 ring-offset-2" : ""
                  }`}
                  style={{ background: c, ["--tw-ring-color" as string]: c }}
                >
                  {p.accent === c && <Check size={10} strokeWidth={3} className="text-white" />}
                </button>
              ))}
            </div>
          </Row>
          <Row label="材质" desc="玻璃：窗口透出桌面，各层跟着透明度变">
            <Segmented
              value={p.material}
              options={[
                { value: "flat", label: "平面" },
                { value: "glass", label: "玻璃" },
              ]}
              onChange={(v) => set("material", v)}
            />
          </Row>
          {p.material === "glass" && (
            <>
              <Row label="玻璃样式" desc="透明、标准是 macOS 26 的 Liquid Glass；更早的系统用磨砂">
                <Segmented
                  value={p.glassStyle}
                  options={[
                    { value: "clear", label: "透明" },
                    { value: "regular", label: "标准" },
                    { value: "frosted", label: "磨砂" },
                  ]}
                  onChange={(v) => set("glassStyle", v)}
                />
              </Row>
              <Row label="透明度" desc="侧栏、会话区、卡片一起变；菜单和对话框保持读得清">
                <Range
                  label="玻璃透明度"
                  value={p.glassClarity}
                  // 拖动时连续写偏好，不弹「已保存」。
                  onChange={(v) => prefs.set("glassClarity", v)}
                />
              </Row>
            </>
          )}
          <Row label="界面字号">
            <Segmented
              value={p.fontSize}
              options={[
                { value: "sm", label: "小" },
                { value: "md", label: "标准" },
                { value: "lg", label: "大" },
              ]}
              onChange={(v) => set("fontSize", v)}
            />
          </Row>
        </Card>
      </Section>

      <Section title="字体与代码">
        <Card>
          <Row label="对话字体" desc="回复和你发的消息；界面其它地方不变">
            <Select
              width={190}
              value={p.chatFont}
              options={Object.entries(CHAT_FONTS).map(([value, f]) => ({ value, label: f.label }))}
              onChange={(v) => set("chatFont", v as ChatFont)}
            />
          </Row>
          <Row label="代码字体" desc="代码块、行内代码、工具卡">
            <Select
              width={190}
              value={p.codeFont}
              options={Object.entries(CODE_FONTS).map(([value, f]) => ({ value, label: f.label }))}
              onChange={(v) => set("codeFont", v as CodeFont)}
            />
          </Row>
          <Row label="代码高亮配色">
            <Select
              width={190}
              value={p.codeTheme}
              options={CODE_THEMES}
              onChange={(v) => {
                preloadTheme(v);
                set("codeTheme", v);
              }}
            />
          </Row>
          <div className="bg-pane px-5 py-4 select-text">
            <Markdown text={SAMPLE} />
          </div>
        </Card>
      </Section>

      <Section title="对话">
        <Card>
          <Row label="显示消息时间戳" desc="在你发的消息旁显示发送时间">
            <Toggle on={p.timestamps} onChange={(v) => set("timestamps", v)} />
          </Row>
          <Row label="默认展开思考过程" desc="支持推理的模型想完后不折起来">
            <Toggle on={p.expandReasoning} onChange={(v) => set("expandReasoning", v)} />
          </Row>
          <Row label="工具卡默认展开" desc="工具调用结果默认展开显示">
            <Toggle on={p.expandTools} onChange={(v) => set("expandTools", v)} />
          </Row>
        </Card>
      </Section>

      <Section title="侧栏">
        <Card>
          <Row label="启动时折叠侧栏" desc="整列收起，只留左上角的展开按钮；⌘B 随时切换">
            <Toggle on={p.sidebarCollapsed} onChange={(v) => set("sidebarCollapsed", v)} />
          </Row>
        </Card>
      </Section>
    </Page>
  );
}

function ThemeCard({
  label,
  selected,
  soon,
  children,
}: {
  label: string;
  selected?: boolean;
  soon?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div title={soon ? COMING_SOON : undefined} className={`flex w-[120px] flex-col items-center gap-1.5 ${soon ? "cursor-not-allowed" : ""}`}>
      <div
        className={`relative h-16 w-full rounded-lg p-1 ${selected ? "border-2 border-accent" : "border border-[#d1d1d6]"} ${soon ? "opacity-60" : ""}`}
      >
        {children}
        {soon && (
          <span className="absolute right-1.5 top-1.5 rounded bg-[#ff9f0a] px-1 text-[9px] font-semibold text-white">即将推出</span>
        )}
      </div>
      <span className={`text-[12px] ${selected ? "font-medium text-ink" : "text-ink-3"}`}>{label}</span>
    </div>
  );
}

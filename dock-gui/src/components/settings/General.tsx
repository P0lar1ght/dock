// 常规（goy）：新对话默认值、启动、通知、数据目录。都是 GUI 自己的偏好（dock/prefs.ts），
// 只有数据目录是从 Dock 读的。

import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useApp } from "../../dock/store";
import { prefs, usePrefs, type Prefs } from "../../dock/prefs";
import { configStatus } from "../../dock/settingsApi";
import { pickDirectory } from "../pickDirectory";
import { Card, Page, Row, Section, Segmented, Select, TextInput, Toggle, flashSaved, secondaryButton, smallButton, useLoad } from "./ui";

/** 和 App 的品牌行下拉共用：新对话默认预设。 */
export const NEW_CHAT_PRESET_KEY = "dock-gui.newChatPreset";

export function GeneralPane() {
  const p = usePrefs();
  const app = useApp();
  const status = useLoad(configStatus);
  const saved = localStorage.getItem(NEW_CHAT_PRESET_KEY);
  const preset = saved && app.presets.some((x) => x.id === saved && x.available) ? saved : (app.defaultPreset ?? "");
  const set = <K extends keyof Prefs>(k: K, v: Prefs[K]) => {
    prefs.set(k, v);
    flashSaved();
  };
  const home = status.data?.dockHome ?? "";

  return (
    <Page title="常规" subtitle="新对话、启动与通知">
      <Section title="新对话">
        <Card>
          <Row label="默认预设" desc="侧栏品牌行的下拉也改这一项">
            <Select
              value={preset}
              options={app.presets.filter((x) => x.available).map((x) => ({ value: x.id, label: x.label }))}
              onChange={(id) => {
                localStorage.setItem(NEW_CHAT_PRESET_KEY, id);
                window.dispatchEvent(new CustomEvent("dock:new-chat-preset", { detail: id }));
                flashSaved();
              }}
            />
          </Row>
          <Row
            label="默认审批"
            desc={
              p.approval === "auto" ? (
                <span className="text-[#c93400]">「始终允许」有风险：bash、文件写入、桌面操作都不再问你</span>
              ) : (
                "新对话里工具要跑之前先问你"
              )
            }
          >
            <Segmented
              value={p.approval}
              options={[
                { value: "ask", label: "询问" },
                { value: "auto", label: "始终允许" },
              ]}
              onChange={(v) => set("approval", v)}
            />
          </Row>
          <Row label="默认思考" desc="模型不支持推理时无效">
            <Toggle on={p.thinking} onChange={(v) => set("thinking", v)} />
          </Row>
        </Card>
      </Section>

      <Section title="启动">
        <Card>
          <Row label="启动时恢复上次打开的会话">
            <Toggle on={p.restoreSession} onChange={(v) => set("restoreSession", v)} />
          </Row>
          <Row label="默认项目目录" desc="新对话没有最近会话可跟时，归到这个目录">
            <TextInput
              value={p.projectDir}
              onCommit={(v) => set("projectDir", v.trim())}
              placeholder="不指定"
              mono
              width={220}
            />
            <button
              type="button"
              onClick={async () => {
                const dir = await pickDirectory(p.projectDir || undefined).catch(() => null);
                if (dir) set("projectDir", dir);
              }}
              className={secondaryButton}
            >
              选择…
            </button>
          </Row>
        </Card>
      </Section>

      <Section title="通知">
        <Card>
          <Row label="一轮完成时通知">
            <Toggle on={p.notifyTurnDone} onChange={(v) => set("notifyTurnDone", v)} />
          </Row>
          <Row label="需要审批 / 回答问题时通知">
            <Toggle on={p.notifyNeedsInput} onChange={(v) => set("notifyNeedsInput", v)} />
          </Row>
          <Row label="只在窗口不在前台时通知">
            <Toggle on={p.notifyBackgroundOnly} onChange={(v) => set("notifyBackgroundOnly", v)} />
          </Row>
        </Card>
      </Section>

      <Section title="数据">
        <Card>
          <Row label="Dock 数据目录" desc={<span className="font-mono">{home || "—"}</span>}>
            {!app.remote && (
              <button type="button" disabled={!home} onClick={() => revealItemInDir(home).catch(() => {})} className={smallButton}>
                在访达中显示
              </button>
            )}
          </Row>
          <Row label="会话存档位置" desc={<span className="font-mono">{home ? `${home}/sessions` : "—"}</span>}>
            {!app.remote && (
              <button
                type="button"
                disabled={!home}
                onClick={() => revealItemInDir(`${home}/sessions`).catch(() => revealItemInDir(home).catch(() => {}))}
                className={smallButton}
              >
                在访达中显示
              </button>
            )}
          </Row>
        </Card>
      </Section>
    </Page>
  );
}

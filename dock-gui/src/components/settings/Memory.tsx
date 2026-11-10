// 记忆（eft）：跨会话记忆的 [memory] 段。默认关；DOCK_MEMORY=0 强制关时整页置灰。
// 改完 Dock 当场重读（记忆工具跟着挂上 / 摘下），不用重启。

import { useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { configStatus } from "../../dock/settingsApi";
import { useApp } from "../../dock/store";
import { useConfigKeys } from "./configKeys";
import {
  Banner,
  Card,
  EnvPill,
  ErrorLine,
  Loading,
  Page,
  Row,
  Section,
  Segmented,
  TextInput,
  Toggle,
  secondaryButton,
} from "./ui";

const int = (v: string) => {
  const n = Number(v.trim());
  return v.trim() && Number.isInteger(n) && n >= 0 ? n : null;
};

export function MemoryPane() {
  // 连远程 Dock 时记忆目录在远端，本机访达打不开。
  const remote = useApp().remote;
  const c = useConfigKeys();
  const [keyMode, setKeyMode] = useState<"env" | "inline" | null>(null);
  const [keyDraft, setKeyDraft] = useState("");
  if (c.loading) return <Loading />;
  const forced = c.data?.memoryForcedOff ?? false;
  const envMemory = c.data?.env.DOCK_MEMORY ?? null;
  const enabled = c.get<boolean>("memory.enabled") ?? false;
  const off = forced || !enabled;
  const hasKey = c.get<boolean>("memory.embedding.api_key") === true;
  const mode = keyMode ?? (hasKey ? "inline" : "env");

  const intRow = (key: string, label: string, desc: string, placeholder: string, suffix: string) => (
    <Row label={label} desc={desc} disabled={off}>
      <TextInput
        value={c.get<number>(key)?.toString() ?? ""}
        onCommit={(v) => void c.write(key, int(v))}
        placeholder={placeholder}
        suffix={suffix}
        width={130}
        disabled={off}
      />
    </Row>
  );

  return (
    <Page
      title="记忆"
      subtitle="跨会话记忆默认关闭；启用后可自动整理与归纳"
      actions={
        !remote && (
          <button
            type="button"
            onClick={() => {
              configStatus().then((s) => revealItemInDir(`${s.dockHome}/memory`).catch(() => revealItemInDir(s.dockHome)));
            }}
            className={secondaryButton}
          >
            浏览记忆…
          </button>
        )
      }
      banner={forced && <Banner kind="warn">被 DOCK_MEMORY={envMemory ?? "0"} 强制关闭 · 开关已禁用</Banner>}
    >
      {c.error && <ErrorLine>读不到配置：{c.error}</ErrorLine>}
      <Section title="总开关">
        <Card>
          <Row
            label="启用跨会话记忆"
            desc={forced ? `被 DOCK_MEMORY=${envMemory ?? "0"} 强制关闭` : "开了之后 agent 能用 memory_search / memory_get 查以前的会话"}
          >
            {envMemory !== null && <EnvPill name="DOCK_MEMORY" />}
            <Toggle on={enabled && !forced} disabled={forced} onChange={(v) => void c.write("memory.enabled", v)} />
          </Row>
        </Card>
      </Section>

      <Section title="自动整理 flush">
        <Card>
          <Row label="启用自动整理" desc="上下文快满时把会话要点写进记忆" disabled={off}>
            <Toggle
              on={c.get<boolean>("memory.flush.enabled") ?? true}
              disabled={off}
              onChange={(v) => void c.write("memory.flush.enabled", v ? null : false)}
            />
          </Row>
          {intRow("memory.flush.soft_threshold_tokens", "触发阈值", "离压缩还剩这么多 token 时整理一次", "4000", "token")}
          {intRow("memory.flush.max_flush_write_chars", "单次写入上限", "一次整理最多写多少字", "8000", "字")}
        </Card>
      </Section>

      <Section title="定期归纳 dream">
        <Card>
          <Row label="启用定期归纳" desc="后台把零散的记录归纳成长期记忆" disabled={off}>
            <Toggle
              on={c.get<boolean>("memory.dream.enabled") ?? true}
              disabled={off}
              onChange={(v) => void c.write("memory.dream.enabled", v ? null : false)}
            />
          </Row>
          {intRow("memory.dream.min_hours", "至少间隔", "两次归纳之间最短间隔", "24", "小时")}
          {intRow("memory.dream.min_sessions", "至少会话数", "累积这么多会话后再归纳", "5", "个")}
        </Card>
      </Section>

      <Section title="向量检索（可选）">
        <Card>
          <Row label="嵌入模型" desc="不配时只用全文检索" disabled={off}>
            <TextInput
              value={c.get<string>("memory.embedding.model") ?? ""}
              onCommit={(v) => void c.write("memory.embedding.model", v.trim() || null)}
              placeholder="text-embedding-3-small"
              width={220}
              disabled={off}
            />
          </Row>
          <Row label="地址" desc="嵌入 API 地址" disabled={off}>
            <TextInput
              value={c.get<string>("memory.embedding.base") ?? ""}
              onCommit={(v) => void c.write("memory.embedding.base", v.trim() || null)}
              placeholder="https://api.openai.com/v1"
              mono
              width={220}
              disabled={off}
            />
          </Row>
          {intRow("memory.embedding.dimensions", "维度", "向量维度", "1024", "")}
          <Row label="密钥来源" disabled={off}>
            <Segmented
              value={mode}
              disabled={off}
              options={[
                { value: "env", label: "环境变量" },
                { value: "inline", label: "直接填写" },
              ]}
              onChange={(v) => {
                setKeyMode(v);
                if (v === "env" && hasKey) void c.write("memory.embedding.api_key", null);
              }}
            />
          </Row>
          {mode === "env" ? (
            <Row label="密钥" desc="读 DOCK_MEMORY_EMBEDDING_API_KEY，没有再读 OPENAI_API_KEY" disabled={off} />
          ) : (
            <Row
              label="密钥"
              desc={<span className="text-[#c93400]">会以明文写进 config.toml，建议用环境变量</span>}
              disabled={off}
            >
              <TextInput
                value={keyDraft}
                onChange={setKeyDraft}
                type="password"
                mono
                width={220}
                disabled={off}
                placeholder={hasKey ? "已保存（留空 = 不改）" : "sk-…"}
              />
              <button
                type="button"
                disabled={off || !keyDraft.trim()}
                onClick={async () => {
                  if (await c.write("memory.embedding.api_key", keyDraft.trim())) setKeyDraft("");
                }}
                className={secondaryButton}
              >
                保存
              </button>
            </Row>
          )}
        </Card>
      </Section>
    </Page>
  );
}

// 个性化（参考用户给的截图）：个人资料、自定义指令、关于你、记忆。
// - 个人资料（名字、头像）是 GUI 偏好，只在本机界面上显示，不发给模型。
// - 自定义指令 = ~/.dock/AGENTS.md（每个工作区都带），关于你 = ~/.dock/USER.md；
//   Dock 把两份放进同一个规约提醒块注入消息流，改完下一步就按新内容来。
// - 记忆的两个开关和记忆页同一组键（memory.enabled / memory.flush.enabled）。

import { useEffect, useRef, useState } from "react";
import { ImagePlus, Info, LoaderCircle } from "lucide-react";
import { Avatar, avatarFromFile } from "../Avatar";
import { displayName, prefs, usePrefs } from "../../dock/prefs";
import { errorText, profileGet, profileSet, type Profile } from "../../dock/settingsApi";
import { useConfigKeys } from "./configKeys";
import { Card, ErrorLine, Loading, Page, Row, Section, TextInput, Toggle, flashSaved, secondaryButton, smallButton, useLoad } from "./ui";

export function PersonalizationPane({ onOpenMemory }: { onOpenMemory: () => void }) {
  const p = usePrefs();
  const profile = useLoad(profileGet);
  const c = useConfigKeys();
  const fileRef = useRef<HTMLInputElement>(null);
  const [avatarError, setAvatarError] = useState<string | null>(null);

  const forced = c.data?.memoryForcedOff ?? false;
  const memoryOn = (c.get<boolean>("memory.enabled") ?? false) && !forced;

  return (
    <Page title="个性化" subtitle="让 Dock 了解你、按你的方式工作">
      <Section title="个人资料">
        <Card>
          <Row label="头像" desc={avatarError ? <span className="text-[#ff3b30]">{avatarError}</span> : "显示在左下角，只存在本机"}>
            <Avatar size={36} />
            <button type="button" onClick={() => fileRef.current?.click()} className={smallButton}>
              <ImagePlus size={12} />
              选择图片
            </button>
            {p.avatar && (
              <button type="button" onClick={() => prefs.set("avatar", "")} className={smallButton}>
                移除
              </button>
            )}
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              className="hidden"
              onChange={async (e) => {
                const file = e.target.files?.[0];
                e.target.value = "";
                if (!file) return;
                setAvatarError(null);
                try {
                  prefs.set("avatar", await avatarFromFile(file));
                  flashSaved();
                } catch (err) {
                  setAvatarError(errorText(err));
                }
              }}
            />
          </Row>
          <Row label="名字" desc="只在界面上显示；想让 Agent 知道，写进下面的「关于你」">
            <TextInput
              value={p.profileName}
              onCommit={(v) => {
                prefs.set("profileName", v.trim());
                flashSaved();
              }}
              placeholder={displayName({ ...p, profileName: "" })}
              width={200}
            />
          </Row>
        </Card>
      </Section>

      {profile.error && <ErrorLine>读不到个性化设置：{profile.error}（Dock 版本太旧？）</ErrorLine>}
      {profile.loading && !profile.data && <Loading />}
      {profile.data && (
        <>
          <ProfileText
            kind="instructions"
            title="自定义指令"
            hint="每个对话都会带上，优先级高于 Agent 的默认习惯；项目里的 AGENTS.md 接在它后面。"
            placeholder="定义 Agent 应该如何工作、回答和执行任务，为所有项目里的 Agent 提供额外指令和上下文…"
            profile={profile.data}
            onSaved={profile.set}
          />
          <ProfileText
            kind="about"
            title="关于你"
            hint="你的背景、角色和长期偏好；和自定义指令一起交给 Agent。"
            placeholder="告诉 Agent 你的背景和长期偏好……"
            profile={profile.data}
            onSaved={profile.set}
          />
        </>
      )}

      <Section title="记忆">
        <Card>
          <Row label="记忆" desc={forced ? "被 DOCK_MEMORY 环境变量强制关闭" : "跨会话记住你和项目：agent 能查以前的会话"}>
            <Toggle on={memoryOn} disabled={forced || c.loading} label="记忆" onChange={(v) => void c.write("memory.enabled", v)} />
          </Row>
          <Row label="主动记忆" desc="上下文快满时主动把值得长期保留的偏好和经验写进记忆" disabled={!memoryOn}>
            <Toggle
              on={memoryOn && (c.get<boolean>("memory.flush.enabled") ?? true)}
              disabled={!memoryOn}
              label="主动记忆"
              onChange={(v) => void c.write("memory.flush.enabled", v ? null : false)}
            />
          </Row>
          <Row label="记忆管理" desc="定期归纳、向量检索、浏览已记下的内容">
            <button type="button" onClick={onOpenMemory} className={secondaryButton}>
              管理
            </button>
          </Row>
        </Card>
      </Section>
    </Page>
  );
}

/** 一份文本：改了才出「保存」，存完回 Dock 的整份内容。 */
function ProfileText({
  kind,
  title,
  hint,
  placeholder,
  profile,
  onSaved,
}: {
  kind: "instructions" | "about";
  title: string;
  hint: string;
  placeholder: string;
  profile: Profile;
  onSaved: (p: Profile) => void;
}) {
  const file = profile[kind];
  const [draft, setDraft] = useState(file.text);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 文件被别处改了（重新读到的内容变了）：草稿跟着换。
  useEffect(() => setDraft(file.text), [file.text]);
  const dirty = draft.trimEnd() !== file.text.trimEnd();

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      onSaved(await profileSet(kind, draft));
      flashSaved();
    } catch (e) {
      setError(errorText(e));
    }
    setSaving(false);
  };

  return (
    <Section
      title={title}
      right={
        <button
          type="button"
          disabled={!dirty || saving}
          onClick={() => void save()}
          className="motion-colors flex items-center gap-1 rounded-md px-2 py-0.5 text-[12px] font-medium text-accent hover:bg-[#0a84ff14] disabled:text-ink-5 disabled:hover:bg-transparent"
        >
          {saving && <LoaderCircle size={12} className="animate-spin" />}
          保存
        </button>
      }
    >
      <textarea
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "s") {
            e.preventDefault();
            if (dirty) void save();
          }
        }}
        placeholder={placeholder}
        spellCheck={false}
        className="md-surface thin-scroll min-h-[150px] resize-y rounded-xl border border-line-soft bg-[#f5f5f7] px-4 py-3 text-[13px] leading-[1.6] text-ink outline-none transition-colors duration-[var(--dur-base)] placeholder:text-ink-4 focus:border-accent focus:bg-white"
      />
      <span className="flex items-start gap-1 text-[11.5px] text-ink-4">
        <Info size={12} className="mt-[2px] shrink-0" />
        <span>
          {hint} 存在 <span className="font-mono">{file.path}</span>，⌘S 保存。
        </span>
      </span>
      {error && <ErrorLine>保存失败：{error}</ErrorLine>}
    </Section>
  );
}

// 输入框。设计：wonder-ui/chat/{ComposerV2,ComposerRunning,Images}。
// Enter 发送、Shift+Enter 换行，输入法组字时不发送。运行中 Enter 排队（排队条上可改插话）、
// ⌘Enter 停止并发送。
// 「+」/ 粘贴 / 拖进来加图（最多 4 张）；「/」开头弹命令下拉，整行交给 Dock 的 slash/execute。
// 「目标」胶囊（或 /goal）：这一条发出去就立成目标；模型左边的圆环看上下文占用（或 /context）。
// 设计：wonder-ui/chat/ComposerGoalContext。

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
  type KeyboardEvent,
} from "react";
import { ArrowUp, ChevronDown, Goal, Hand, ListChecks, Mic, Plus, X } from "lucide-react";
import type { ComposerSettings, ContextBreakdown, ImageInput, SlashCommand, SlashResult } from "../model";
import { store, useGatewayId, type SendMode } from "../dock/store";
import { COMING_SOON } from "./chrome";
import { ContextRing } from "./ContextRing";
import { useComposerItems } from "../features/registry";
import { ImageViewer } from "./ImageViewer";
import { MAX_IMAGES, describeImage, prepareImage } from "./imageInput";
import { MenuButton } from "./Popover";
import { imeComposing } from "../dock/ime";

const MAX_HEIGHT = 200;

const APPROVAL = [
  { id: "ask", label: "请求批准", hint: "逐次确认" },
  { id: "auto", label: "自动批准", hint: "不再询问" },
];
const EFFORT: Record<string, string> = { "": "默认", none: "关", minimal: "极低", low: "低", medium: "中", high: "高", xhigh: "极高" };

export interface ComposerActions {
  /** 成功回 true，输入框才清空（失败时文字和图都留着，改改再发）。 */
  onSend: (text: string, images: ImageInput[], mode: SendMode) => Promise<boolean>;
  onStop: () => void;
  onSetModel: (modelId: string) => void;
  onSetApproval: (mode: string) => void;
  onSetReasoning: (effort: string) => void;
  onSetPlan: (enabled: boolean) => void;
  /** 命令表（第一次敲「/」时拉）。 */
  loadSlash: () => Promise<SlashCommand[]>;
  /** 跑一行斜杠命令；`null` = 出错了（错误已经显示在别处）。 */
  onSlash: (text: string) => Promise<SlashResult | null>;
  /** 把这一条立成目标发出去；成功回 true。 */
  onStartGoal: (text: string, images: ImageInput[]) => Promise<boolean>;
  /** 上下文圆环点开时拉明细。 */
  loadContext: () => Promise<ContextBreakdown>;
  onCompact: () => void;
}

const GOAL_PLACEHOLDER = "描述你的目标，定义可衡量的成果，以获得最佳效果";

/** 下拉里的一项：命令，或者 /model、/effort 后面的可选值。 */
interface Choice {
  key: string;
  label: string;
  desc: string;
  group: string;
  run: () => void;
}

const chip = "flex h-7 items-center gap-1 rounded-full bg-[#f5f5f7] px-2.5 text-xs font-medium text-ink-3 hover:bg-[#ececf0]";

/** 切走再回来时留着没发出去的草稿（按网关 + 会话）。图不带走。 */
const drafts = new Map<string, string>();

export function Composer({
  threadId,
  settings,
  running,
  stopping,
  ...on
}: { threadId: string; settings: ComposerSettings; running: boolean; stopping: boolean } & ComposerActions) {
  const composerItems = useComposerItems();
  const gw = useGatewayId();
  const draftId = `${gw}\n${threadId}`;
  const [text, setText] = useState(() => drafts.get(draftId) ?? "");
  const textRef = useRef(text);
  textRef.current = text;
  const draftKey = useRef(draftId);
  if (draftKey.current !== draftId) {
    drafts.set(draftKey.current, textRef.current);
    draftKey.current = draftId;
    setText(drafts.get(draftId) ?? "");
  }
  useEffect(
    () => () => {
      drafts.set(draftId, textRef.current);
    },
    [draftId],
  );
  const [images, setImages] = useState<ImageInput[]>([]);
  const [focused, setFocused] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  // 命令结果 / 图片出错：浮在输入框上方的一张卡（不挤输入框）。短回执几秒后自己收起。
  const [notice, setNotice] = useState<{ title: string; body: string } | null>(null);
  const noticeRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [preview, setPreview] = useState<ImageInput | null>(null);
  const [commands, setCommands] = useState<SlashCommand[] | null>(null);
  const [pick, setPick] = useState(0);
  const [slashClosed, setSlashClosed] = useState(false);
  // 「目标」胶囊开着：这一条发出去就立成目标。
  const [goalArmed, setGoalArmed] = useState(false);
  const [contextOpen, setContextOpen] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // 别的面板填进来的话（如浏览器面板的示例）：换掉当前草稿并聚焦，不发送。
  useEffect(
    () =>
      store.onFill(threadId, (t, mode, imgs) => {
        setText((cur) => (mode === "append" && cur.trim() ? `${cur.replace(/\s*$/, " ")}${t}` : t));
        if (imgs) setImages(imgs);
        ref.current?.focus();
      }),
    [threadId],
  );

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  // 「/」开头、还没敲空格：按前缀（再按包含）筛命令。
  const slashQuery = /^\/[^\s]*$/.test(text) ? text.slice(1).toLowerCase() : null;
  // 「/model 」「/effort 」后面：直接列可选的模型 / 档位，选了就切（和右下角菜单同一个接口）。
  const argMatch = /^\/(model|effort)\s+(\S*)$/.exec(text);
  useEffect(() => {
    if (slashQuery !== null && !commands) on.loadSlash().then(setCommands, () => setCommands([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slashQuery !== null]);

  const finish = (message: string) => {
    setText("");
    setNotice({ title: "", body: message });
  };
  let choices: Choice[] = [];
  if (argMatch) {
    const q = argMatch[2].toLowerCase();
    choices =
      argMatch[1] === "model"
        ? settings.models
            .filter((m) => `${m.id} ${m.label}`.toLowerCase().includes(q))
            .map((m) => ({
              key: m.id,
              label: m.label,
              desc: m.id === settings.modelId ? "当前" : m.id === m.label ? "" : m.id,
              group: "切换模型",
              run: () => {
                on.onSetModel(m.id);
                finish(`已切换到 ${m.label}`);
              },
            }))
        : settings.reasoningOptions
            .filter((e) => e.includes(q) || (EFFORT[e] ?? "").includes(q))
            .map((e) => ({
              key: e,
              label: EFFORT[e] ?? e,
              desc: e === settings.reasoning ? `${e} · 当前` : e,
              group: "推理强度",
              run: () => {
                on.onSetReasoning(e);
                finish(`推理强度：${EFFORT[e] ?? e}`);
              },
            }));
  } else if (slashQuery !== null && commands) {
    const hit = [
      ...commands.filter((c) => c.name.startsWith(slashQuery)),
      ...commands.filter((c) => !c.name.startsWith(slashQuery) && c.name.includes(slashQuery)),
    ];
    // 命令在前，技能 / 工作流在后，各自保持 Dock 给的顺序。
    choices = [...hit.filter((c) => c.kind === "command"), ...hit.filter((c) => c.kind !== "command")].map((c) => ({
      key: c.name,
      label: c.display,
      desc: c.description,
      group: c.kind === "command" ? "命令" : "技能与工作流",
      run: () => complete(c),
    }));
  }
  const slashOpen = choices.length > 0 && !slashClosed;
  useEffect(() => {
    setPick(0);
    setSlashClosed(false);
  }, [slashQuery, argMatch?.[0]]);
  useEffect(() => {
    if (slashOpen) itemRefs.current[pick]?.scrollIntoView({ block: "nearest" });
  }, [pick, slashOpen]);

  // 浮卡：Esc / 点外面收起；短回执（没标题、一行话）三秒后自己收。
  useEffect(() => {
    if (!notice) return;
    const onKey = (e: globalThis.KeyboardEvent) => e.key === "Escape" && setNotice(null);
    const onDown = (e: MouseEvent) => !noticeRef.current?.contains(e.target as Node) && setNotice(null);
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    const short = !notice.title && notice.body.length <= 60 && !notice.body.includes("\n");
    const timer = short ? window.setTimeout(() => setNotice(null), 3000) : undefined;
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
      window.clearTimeout(timer);
    };
  }, [notice]);

  const addFiles = async (files: Iterable<File>) => {
    const list = [...files].filter((f) => f.type.startsWith("image/"));
    if (list.length === 0) return;
    const room = MAX_IMAGES - images.length;
    if (room <= 0) {
      setNotice({ title: "", body: `一条消息最多带 ${MAX_IMAGES} 张图。` });
      return;
    }
    const added: ImageInput[] = [];
    for (const f of list.slice(0, room)) {
      try {
        added.push(await prepareImage(f));
      } catch (e) {
        setNotice({ title: "", body: e instanceof Error ? e.message : String(e) });
      }
    }
    if (list.length > room) setNotice({ title: "", body: `一条消息最多带 ${MAX_IMAGES} 张图，多出来的没加。` });
    setImages((prev) => [...prev, ...added].slice(0, MAX_IMAGES));
  };

  const complete = (c: SlashCommand) => {
    if (c.name === "goal" || c.name === "context") {
      // 在下拉里选中：直接开胶囊 / 开详情，不用再敲回车。
      void submit(`/${c.name}`, "auto");
    } else if (c.name === "model" || c.name === "effort") {
      // 接着列可选项（上面的 argMatch），不关下拉。
      setText(`/${c.name} `);
      ref.current?.focus();
    } else if (c.takesArgs) {
      setText(`/${c.name} `);
      setSlashClosed(true);
      ref.current?.focus();
    } else {
      void submit(`/${c.name}`, "auto");
    }
  };

  const submit = async (raw: string, mode: SendMode) => {
    const trimmed = raw.trim();
    if ((!trimmed && images.length === 0) || busy) return;
    // /help：下拉就是帮助，不去问 Dock（它的帮助文本混着终端专用命令）。
    if (trimmed === "/help" || trimmed === "/?") {
      setText("/");
      setSlashClosed(false);
      ref.current?.focus();
      return;
    }
    // /goal 不带目标：打开「目标」胶囊（Dock 那边只会回一段填空模板）；/context：打开上下文详情。
    if (trimmed === "/goal") {
      setText("");
      setGoalArmed(true);
      ref.current?.focus();
      return;
    }
    if (trimmed === "/context") {
      setText("");
      setContextOpen(true);
      return;
    }
    setNotice(null);
    setBusy(true);
    try {
      if (goalArmed && !trimmed.startsWith("/")) {
        if (await on.onStartGoal(trimmed, images)) {
          setText("");
          setImages([]);
          setGoalArmed(false);
        }
        return;
      }
      // 斜杠命令整行交给 Dock；带着图的消息不当命令。
      if (trimmed.startsWith("/") && images.length === 0) {
        const r = await on.onSlash(trimmed);
        if (!r) return;
        if (r.kind === "filled") {
          setText(r.fill);
          ref.current?.focus();
          return;
        }
        if (r.kind === "notice") setNotice({ title: r.title, body: r.body });
        if (r.kind !== "passthrough") {
          setText("");
          return;
        }
      }
      if (await on.onSend(trimmed, images, mode)) {
        setText("");
        setImages([]);
      }
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (imeComposing(e)) return;
    if (slashOpen) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const step = e.key === "ArrowDown" ? 1 : -1;
        setPick((p) => (p + step + choices.length) % choices.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
        e.preventDefault();
        choices[pick]?.run();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setSlashClosed(true);
        return;
      }
    }
    // 空着按退格：收起「目标」胶囊（和删掉一个标签一样）。
    if (e.key === "Backspace" && goalArmed && !text && images.length === 0) {
      e.preventDefault();
      setGoalArmed(false);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void submit(text, running && (e.metaKey || e.ctrlKey) ? "stop" : "auto");
    }
  };

  const onPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...e.clipboardData.files].filter((f) => f.type.startsWith("image/"));
    if (files.length === 0) return;
    e.preventDefault();
    void addFiles(files);
  };

  const onDrop = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setDragging(false);
    void addFiles(e.dataTransfer.files);
  };

  const approval = APPROVAL.find((a) => a.id === settings.approval) ?? APPROVAL[0];
  // 已经有进行中 / 暂停的目标：目标条在上面管它，胶囊不再出现。
  const hasGoal = settings.goal !== null && settings.goal.status !== "completed";
  const canSend = (!!text.trim() || images.length > 0) && !busy;

  return (
    <div
      // @container：分屏时输入框窄，底栏按自身宽度收起文字（审批只留图标、「推理」前缀、模型名截短）。
      onDragOver={(e) => {
        if (![...e.dataTransfer.types].includes("Files")) return;
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setDragging(false)}
      onDrop={onDrop}
      className={`glass-card @container relative mx-4 mb-3 flex shrink-0 flex-col gap-3 rounded-[22px] border bg-white px-4 pb-3 pt-3.5 transition-[box-shadow,border-color,transform] duration-[var(--dur-base)] ease-[var(--ease-out)] ${
        dragging
          ? "border-accent shadow-[0_0_0_3px_rgba(10,132,255,0.15)]"
          : focused
            ? "-translate-y-0.5 border-[#d1d1d6] shadow-[0px_10px_32px_rgba(0,0,0,0.13)]"
            : "border-line-strong shadow-[0px_8px_28px_rgba(0,0,0,0.1)]"
      }`}
    >
      {slashOpen ? (
        <div
          role="listbox"
          aria-label="斜杠命令"
          className="glass-popover pop-in thin-scroll absolute inset-x-0 bottom-[calc(100%+8px)] z-20 flex max-h-80 flex-col overflow-y-auto rounded-[14px] border border-line-strong bg-white/95 p-1.5 shadow-[0px_8px_28px_rgba(0,0,0,0.12)] backdrop-blur-xl"
          style={{ transformOrigin: "bottom left" }}
        >
          {choices.map((c, i) => (
            <div key={`${c.group}:${c.key}`} className="flex flex-col">
              {(i === 0 || choices[i - 1].group !== c.group) && (
                <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-semibold text-ink-4">{c.group}</div>
              )}
              <button
                ref={(el) => {
                  itemRefs.current[i] = el;
                }}
                type="button"
                role="option"
                aria-selected={i === pick}
                onMouseEnter={() => setPick(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={c.run}
                className={`flex items-baseline gap-3 rounded-lg px-2.5 py-1.5 text-left ${i === pick ? "bg-[#0a84ff14]" : ""}`}
              >
                <span className="shrink-0 font-mono text-[13px] text-ink">{c.label}</span>
                <span className="min-w-0 truncate text-xs text-ink-4">{c.desc}</span>
              </button>
            </div>
          ))}
          <div className="glass-popover-fill sticky -bottom-1.5 bg-white/95 px-2.5 pb-0.5 pt-1.5 text-[11px] text-ink-5">
            ↑↓ 选择 · Tab / Enter {argMatch ? "确定" : "补全"} · Esc 收起
          </div>
        </div>
      ) : (
        notice && (
          <div
            ref={noticeRef}
            role="status"
            className="glass-popover pop-in absolute inset-x-0 bottom-[calc(100%+8px)] z-20 flex items-start gap-2 rounded-[14px] border border-line-strong bg-white/95 px-3.5 py-2.5 shadow-[0px_8px_28px_rgba(0,0,0,0.12)] backdrop-blur-xl"
            style={{ transformOrigin: "bottom left" }}
          >
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              {notice.title && <span className="text-xs font-semibold text-ink">{notice.title}</span>}
              <span className="thin-scroll max-h-60 overflow-y-auto whitespace-pre-wrap break-words text-[13px] leading-[1.55] text-ink-2 select-text">
                {notice.body}
              </span>
            </div>
            <button
              type="button"
              title="收起 Esc"
              onClick={() => setNotice(null)}
              className="motion-colors flex size-5 shrink-0 items-center justify-center rounded text-ink-4 hover:bg-hover hover:text-ink"
            >
              <X size={12} />
            </button>
          </div>
        )
      )}

      {images.length > 0 && (
        <div className="flex items-center gap-3 pt-0.5">
          {images.map((img) => (
            <div key={img.url} className="pop-in relative size-14 shrink-0">
              <button
                type="button"
                title={describeImage(img)}
                onClick={() => setPreview(img)}
                className="size-full overflow-hidden rounded-md border border-line-strong bg-fill"
              >
                <img src={img.url} alt="" className="size-full object-cover" />
              </button>
              <button
                type="button"
                title="移除"
                onClick={() => setImages((prev) => prev.filter((x) => x !== img))}
                className="absolute -right-1.5 -top-1.5 flex size-4 items-center justify-center rounded-full bg-ink text-white shadow"
              >
                <X size={10} strokeWidth={2.5} />
              </button>
            </div>
          ))}
        </div>
      )}

      <textarea
        ref={ref}
        rows={1}
        value={text}
        placeholder={
          goalArmed
            ? GOAL_PLACEHOLDER
            : running
              ? "追加指令：Enter 本轮结束后发送，⌘Enter 打断并立即发送"
              : "随心输入，/ 看命令，可粘贴图片"
        }
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        className="thin-scroll min-w-0 resize-none bg-transparent text-[15px] leading-snug text-ink outline-none placeholder:text-ink-5 select-text"
      />
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <button
            type="button"
            title={`添加图片（最多 ${MAX_IMAGES} 张，也可以粘贴或拖进来）`}
            onClick={() => fileRef.current?.click()}
            disabled={images.length >= MAX_IMAGES}
            className="motion-colors flex size-7 shrink-0 items-center justify-center rounded-full bg-[#f5f5f7] text-ink-3 hover:bg-[#ececf0] disabled:opacity-40"
          >
            <Plus size={14} />
          </button>
          <input
            ref={fileRef}
            type="file"
            accept="image/*"
            multiple
            hidden
            onChange={(e) => {
              if (e.target.files) void addFiles(e.target.files);
              e.target.value = "";
            }}
          />
          <MenuButton
            title="审批模式（只改这个会话）"
            align="left"
            header="审批模式"
            options={APPROVAL}
            value={approval.id}
            onPick={on.onSetApproval}
            className={`${chip} gap-1.5`}
            trigger={
              <>
                <Hand size={14} strokeWidth={1.5} />
                <span className="hidden @[36rem]:inline">{approval.label}</span>
                <ChevronDown size={11} className="text-ink-4" />
              </>
            }
          />
          <button
            type="button"
            aria-pressed={settings.plan}
            title={settings.plan ? "计划模式开着：先出计划、你批准后再动手（点一下关掉）" : "计划模式：先出计划、你批准后再动手"}
            onClick={() => on.onSetPlan(!settings.plan)}
            className={`motion-colors flex h-7 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium ${
              settings.plan ? "bg-[#0a84ff1a] text-accent hover:bg-[#0a84ff26]" : "bg-[#f5f5f7] text-ink-3 hover:bg-[#ececf0]"
            }`}
          >
            <ListChecks size={14} strokeWidth={1.75} />
            <span className="hidden @[36rem]:inline">计划</span>
          </button>
          {(goalArmed || !hasGoal) && (
            <button
              type="button"
              aria-pressed={goalArmed}
              title={goalArmed ? "这一条发出去就立成目标，一直做到完成（点一下取消）" : "目标：设一个目标，一直做到完成为止"}
              onClick={() => {
                setGoalArmed((v) => !v);
                ref.current?.focus();
              }}
              className={`motion-colors flex h-7 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium ${
                goalArmed ? "bg-[#0a84ff1a] text-accent hover:bg-[#0a84ff26]" : "bg-[#f5f5f7] text-ink-3 hover:bg-[#ececf0]"
              }`}
            >
              <Goal size={14} strokeWidth={1.75} />
              <span className="hidden @[36rem]:inline">目标</span>
            </button>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {/* 功能登记的工具栏小部件（插件状态项），见 features/registry.ts。 */}
          {composerItems.map((Item, i) => (
            <Item key={i} threadId={threadId} />
          ))}
          <ContextRing
            usage={settings.context}
            open={contextOpen}
            onOpenChange={setContextOpen}
            load={on.loadContext}
            onCompact={on.onCompact}
          />
          {settings.models.length > 0 && (
            <MenuButton
              title="模型（只改这个会话）"
              header="模型"
              options={settings.models.map((m) => ({ id: m.id, label: m.label }))}
              value={settings.modelId}
              onPick={on.onSetModel}
              className={chip}
              trigger={
                <>
                  <span className="max-w-[110px] truncate @[36rem]:max-w-[180px]">{settings.model || "模型"}</span>
                  <ChevronDown size={11} className="text-ink-4" />
                </>
              }
            />
          )}
          {settings.reasoningOptions.length > 0 && (
            <MenuButton
              title="推理强度（只改这个会话）"
              header="推理强度"
              options={settings.reasoningOptions.map((e) => ({ id: e, label: EFFORT[e] ?? e, hint: e }))}
              value={settings.reasoning}
              onPick={on.onSetReasoning}
              className={chip}
              trigger={
                <>
                  <span className="hidden font-normal text-ink-4 @[30rem]:inline">推理</span>
                  {EFFORT[settings.reasoning] ?? settings.reasoning}
                  <ChevronDown size={11} className="text-ink-4" />
                </>
              }
            />
          )}
          <button
            type="button"
            title={`语音输入 · ${COMING_SOON}`}
            className="motion-colors flex size-7 shrink-0 items-center justify-center rounded-full bg-[#f5f5f7] text-ink-3 hover:bg-[#ececf0]"
          >
            <Mic size={14} />
          </button>
          {running && !canSend ? (
            <button
              type="button"
              title={stopping ? "正在停止…" : "停止本轮"}
              aria-label="停止"
              onClick={on.onStop}
              disabled={stopping}
              className="pop-in flex size-8 shrink-0 items-center justify-center rounded-full bg-ink transition-[transform,opacity] active:scale-90 disabled:opacity-40"
            >
              {/* 正在停止时里面的方块脉冲；写在按钮上会被 pop-in 的 animation 盖掉。 */}
              <span className={`size-2.5 rounded-[2px] bg-white ${stopping ? "animate-pulse" : ""}`} />
            </button>
          ) : (
            <>
              <button
                type="button"
                title={running ? "排队 Enter：本轮结束后发送（排队条上可改插话）· ⌘Enter 停止并发送" : "发送 Enter"}
                aria-label={running ? "排队" : "发送"}
                onClick={() => void submit(text, "auto")}
                disabled={!canSend}
                className="flex size-8 shrink-0 items-center justify-center rounded-full bg-ink text-white transition-[background-color,transform] duration-[var(--dur-fast)] active:scale-90 disabled:bg-ink-5"
              >
                <ArrowUp size={16} strokeWidth={2.25} />
              </button>
            </>
          )}
        </div>
      </div>
      {preview && <ImageViewer url={preview.url} caption={describeImage(preview)} onClose={() => setPreview(null)} />}
    </div>
  );
}

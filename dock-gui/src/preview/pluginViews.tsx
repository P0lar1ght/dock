// `#plugin-views`：视图树图鉴（假数据），对照 wonder-ui/plugins/PluginViewGallery（hum）。
// 数据先过 dock-core 的 parseView，和真从网关来的一样。

import { useState } from "react";
import { parseView } from "dock-core";
import { ViewTree } from "../features/plugins/ViewTree";
import { StatusItemsView } from "../features/plugins/StatusItems";
import type { StatusItem } from "dock-core";

const label = (text: string) => ({ type: "text", text, tone: "muted", size: "s" });

const GALLERY = parseView({
  type: "stack",
  gap: "l",
  children: [
    label("text"),
    {
      type: "stack",
      gap: "s",
      children: [
        { type: "text", text: "默认正文示例" },
        { type: "text", text: "灰色说明文字", tone: "muted" },
        { type: "text", text: "强调高亮文字", tone: "accent" },
        { type: "text", text: "成功状态文字", tone: "success" },
        { type: "text", text: "警告状态文字", tone: "warning" },
        { type: "text", text: "危险状态文字", tone: "danger" },
        { type: "text", text: "粗体正文", weight: "bold" },
        { type: "text", text: "大号标题", size: "l" },
        { type: "text", text: "小号文字（size s）", size: "s" },
      ],
    },
    label("markdown"),
    { type: "markdown", text: "### 部署说明\n\n- 确认环境变量已写入\n- 使用 `deploy run` 触发" },
    label("code"),
    { type: "code", text: "$ dock deploy --env production\n→ uploading assets…", lang: "sh" },
    label("kv"),
    {
      type: "kv",
      items: [
        { label: "环境", value: "production" },
        { label: "版本", value: "v2.4.1" },
        { label: "状态", value: "运行中", tone: "success" },
        { label: "上次部署", value: "12 分钟前" },
      ],
    },
    label("table"),
    {
      type: "table",
      columns: ["服务", "实例", "状态"],
      rows: [
        ["api-gateway", "3", "健康"],
        ["worker", "2", "部署中"],
        ["web", "4", "健康"],
        ["cache", "1", "降级"],
      ],
    },
    label("list"),
    {
      type: "list",
      items: [
        { title: "v2.4.1", subtitle: "生产 · 12 分钟前", badge: { text: "成功", tone: "success" }, action: "open-241" },
        { title: "v2.4.0", subtitle: "生产 · 昨天 · 悬停可点", badge: { text: "失败", tone: "danger" }, action: "open-240" },
        { title: "v2.3.9", subtitle: "预发 · 3 天前", badge: { text: "进行中", tone: "accent" } },
      ],
    },
    label("badge"),
    {
      type: "row",
      children: ["default", "muted", "accent", "success", "warning", "danger"].map((tone, i) => ({
        type: "badge",
        tone,
        text: ["默认", "灰", "强调", "成功", "警告", "危险"][i],
      })),
    },
    label("progress"),
    { type: "progress", value: 0.6, label: "上传中 3/5" },
    { type: "progress", label: "扫描中" },
    label("button（点「部署」看执行中）"),
    {
      type: "row",
      children: [
        { type: "button", label: "部署", action: "deploy", style: "primary" },
        { type: "button", label: "回滚", action: "rollback" },
        { type: "button", label: "终止", action: "stop", style: "danger" },
      ],
    },
    label("link"),
    { type: "link", label: "查看部署日志", url: "https://example.com/logs" },
    label("section"),
    { type: "section", title: "最近部署", children: [{ type: "text", text: "展开态内容", tone: "muted", size: "s" }] },
    { type: "section", title: "历史版本", collapsed: true, children: [{ type: "text", text: "折叠的内容" }] },
    label("row"),
    {
      type: "row",
      align: "between",
      children: [
        { type: "badge", text: "生产", tone: "accent" },
        { type: "button", label: "回滚", action: "rollback" },
        { type: "button", label: "部署", action: "deploy", style: "primary" },
      ],
    },
    label("divider"),
    { type: "divider" },
    label("empty"),
    { type: "empty", title: "还没有部署记录", action: "deploy", label: "立即部署" },
    label("unsupported"),
    { type: "chart" },
  ],
});

const STATUS: StatusItem[] = [
  { id: "deploy", text: "部署中 60% · 正在滚动更新 api-gateway", tone: "accent", tooltip: "生产环境发布进度，点开看详情", surface: "deploy" },
  { id: "ci", text: "CI 通过", tone: "success", tooltip: null, surface: null },
  { id: "alerts", text: "2 个告警", tone: "warning", tooltip: "生产环境有 2 条告警，点开看详情", surface: "deploy" },
  { id: "lint", text: "构建失败 · lint 3 处错误", tone: "danger", tooltip: "点开查看 lint 报告", surface: "lint" },
  { id: "queue", text: "队列 3", tone: "default", tooltip: "还有 3 个部署在排队", surface: "deploy" },
];

/** 输入框工具栏右组的样子：宽的（最多 3 个 + 「+2」）和分屏时窄的（1 个 + 「+4」）。 */
function StatusRow({ width }: { width: number }) {
  return (
    <div className="@container rounded-[22px] border border-line-strong bg-white px-4 py-3" style={{ width }}>
      <div className="flex items-center justify-end gap-1.5">
        <StatusItemsView items={STATUS} />
        <span className="flex h-7 items-center rounded-full bg-[#f5f5f7] px-2.5 text-xs font-medium text-ink-3">deepseek-flash</span>
      </div>
    </div>
  );
}

export function PluginViewsPreview() {
  const [busy, setBusy] = useState<string | null>(null);
  const onAction = (action: string) => {
    setBusy(action);
    setTimeout(() => setBusy(null), 1500);
  };
  return (
    <div className="flex min-h-screen flex-col gap-6 bg-window p-6">
      <div className="flex flex-col gap-4 pt-24">
        <StatusRow width={860} />
        <StatusRow width={460} />
      </div>
      <div className="w-[420px] rounded-[10px] border border-line-strong bg-pane px-4 py-3">
        <ViewTree node={GALLERY} onAction={onAction} busy={busy} />
      </div>
    </div>
  );
}

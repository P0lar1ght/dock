// 浏览器与联网（cag）：内置浏览器的有头 / 无头 + [toolset.web_fetch]。
// 都是用到时现读：网页抓取下一次调用就按新设置；已开的浏览器不重启，下次拉起才生效。

import { useState } from "react";
import { X } from "lucide-react";
import { browserStatus } from "../../dock/settingsApi";
import { useConfigKeys } from "./configKeys";
import { Banner, Card, EnvPill, ErrorLine, Loading, Page, Pill, Row, Section, TextInput, Toggle, useLoad } from "./ui";
import { imeComposing } from "../../dock/ime";

const int = (v: string) => {
  const n = Number(v.trim());
  return v.trim() && Number.isFinite(n) && n >= 0 ? Math.round(n) : null;
};
const MB = 1024 * 1024;

const STATE: Record<string, { label: string; kind: "green" | "gray" | "red" | "orange" }> = {
  connected: { label: "已连接", kind: "green" },
  disabled: { label: "已停用", kind: "gray" },
  missing: { label: "未启用", kind: "gray" },
  not_mounted: { label: "未挂载", kind: "gray" },
  unreachable: { label: "未连上", kind: "red" },
};

export function BrowserPane() {
  const c = useConfigKeys();
  const status = useLoad(browserStatus);
  const [headedNote, setHeadedNote] = useState(false);
  const [domainDraft, setDomainDraft] = useState("");
  if (c.loading) return <Loading />;

  const st = status.data ? STATE[status.data.state] : null;
  const headedEnv = status.data?.headedEnv ?? !!c.data?.env.DOCK_BROWSER_HEADED;
  const headed = c.get<boolean>("browser.headed") ?? false;
  const domains = c.get<string[]>("toolset.web_fetch.allowed_domains");
  const denyAll = Array.isArray(domains) && domains.length === 0;
  const maxBytes = c.get<number>("toolset.web_fetch.max_content_length");

  const addDomain = () => {
    const d = domainDraft.trim().replace(/^https?:\/\//, "").replace(/\/.*$/, "");
    setDomainDraft("");
    if (!d || (domains ?? []).includes(d)) return;
    void c.write("toolset.web_fetch.allowed_domains", [...(domains ?? []), d]);
  };

  return (
    <Page
      title="浏览器与联网"
      subtitle="内置浏览器与 web_fetch"
      banner={
        headedNote && (
          <Banner kind="info" action={<button type="button" onClick={() => setHeadedNote(false)} className="text-[12px] text-[#0040dd]">知道了</button>}>
            已开的浏览器不重启：所有会话关掉浏览器后，下次拉起时生效
          </Banner>
        )
      }
    >
      {c.error && <ErrorLine>读不到配置：{c.error}</ErrorLine>}
      <Section title="内置浏览器">
        <Card>
          <Row label="状态" desc={status.data?.detail || undefined}>
            {st && <Pill kind={st.kind}>{st.label}</Pill>}
          </Row>
          <Row
            label="显示浏览器窗口（有头）"
            desc={headedEnv ? "DOCK_BROWSER_HEADED 已设：总是有头" : "关 = 无头（看不到窗口，桌面面板里照样能看画面）"}
          >
            {headedEnv && <EnvPill name="DOCK_BROWSER_HEADED" />}
            <Toggle
              on={headed || headedEnv}
              disabled={headedEnv}
              onChange={async (v) => {
                if (await c.write("browser.headed", v)) setHeadedNote(true);
              }}
            />
          </Row>
        </Card>
      </Section>

      <Section title="网页抓取 web_fetch">
        <Card>
          <Row label="超时" desc="整个请求的上限；连接恒 10 秒">
            <TextInput
              value={c.get<number>("toolset.web_fetch.timeout_secs")?.toString() ?? ""}
              onCommit={(v) => void c.write("toolset.web_fetch.timeout_secs", int(v))}
              placeholder="60"
              suffix="秒"
              width={110}
            />
          </Row>
          <Row label="响应体上限" desc="下完才判，超了这次抓取失败">
            <TextInput
              value={maxBytes ? String(+(maxBytes / MB).toFixed(2)) : ""}
              onCommit={(v) => {
                const n = Number(v.trim());
                void c.write("toolset.web_fetch.max_content_length", v.trim() && n > 0 ? Math.round(n * MB) : null);
              }}
              placeholder="10"
              suffix="MB"
              width={110}
            />
          </Row>
          <Row label="给模型的正文上限" desc="超了截断并标 [truncated]">
            <TextInput
              value={c.get<number>("toolset.web_fetch.max_markdown_length")?.toString() ?? ""}
              onCommit={(v) => void c.write("toolset.web_fetch.max_markdown_length", int(v))}
              placeholder="100000"
              suffix="字"
              width={130}
            />
          </Row>
          <Row label="只允许这些域名" desc="留空 = 不限制；要全部拒绝用下面的开关" top disabled={denyAll}>
            <div className="flex w-[300px] flex-wrap items-center justify-end gap-1.5">
              {(domains ?? []).map((d) => (
                <span key={d} className="flex items-center gap-1 rounded-full bg-[#f2f2f7] py-0.5 pl-2 pr-1 text-[11.5px] text-ink-2">
                  {d}
                  <button
                    type="button"
                    aria-label={`移除 ${d}`}
                    onClick={() => {
                      const next = (domains ?? []).filter((x) => x !== d);
                      void c.write("toolset.web_fetch.allowed_domains", next.length ? next : null);
                    }}
                    className="rounded-full p-0.5 text-ink-4 hover:bg-hover hover:text-ink"
                  >
                    <X size={10} />
                  </button>
                </span>
              ))}
              <input
                value={domainDraft}
                disabled={denyAll}
                onChange={(e) => setDomainDraft(e.target.value)}
                onKeyDown={(e) => {
                  if ((e.key === "Enter" || e.key === ",") && !imeComposing(e)) {
                    e.preventDefault();
                    addDomain();
                  }
                }}
                onBlur={addDomain}
                placeholder="加一个域名，回车"
                className="h-7 w-[140px] rounded-lg border border-line-strong bg-white px-2 text-[12px] outline-none focus:border-accent"
              />
            </div>
          </Row>
          <Row label="全部拒绝" desc="开了 web_fetch 一个网页都取不了（web_search 不受影响）">
            <Toggle
              on={denyAll}
              onChange={(v) => void c.write("toolset.web_fetch.allowed_domains", v ? [] : null)}
            />
          </Row>
          <Row
            label="转发代理"
            desc={<span className="text-[#c93400]">设了之后代理成为信任边界：它会替你去连内网</span>}
          >
            <TextInput
              value={c.get<string>("toolset.web_fetch.proxy_endpoint") ?? ""}
              onCommit={(v) => void c.write("toolset.web_fetch.proxy_endpoint", v.trim() || null)}
              placeholder="http://127.0.0.1:7890"
              mono
              width={220}
            />
          </Row>
          <Row label="允许访问 localhost" desc="只放行 localhost / 127.0.0.0/8 / ::1；私有网段和云元数据地址永不放行">
            <Toggle
              on={c.get<boolean>("toolset.web_fetch.allow_local") ?? false}
              onChange={(v) => void c.write("toolset.web_fetch.allow_local", v ? true : null)}
            />
          </Row>
        </Card>
      </Section>
    </Page>
  );
}

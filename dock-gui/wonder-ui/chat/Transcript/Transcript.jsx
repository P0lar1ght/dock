<div data-node-id="his.peto" data-node-label="Feed Scroll Body" className="flex min-h-0 flex-col pt-3 pb-2 flex-1 px-5 gap-3 overflow-clip">
  <div data-node-id="his.shop" data-node-label="Duration Meta" className="w-fit shrink-0 text-[#8e8e93] font-sans text-xs leading-normal text-left block [overflow-wrap:normal]">用时 14m 08s</div>
  <div data-node-id="his.puja" data-node-label="Agent Narrative Block" className="flex h-fit flex-col gap-2">
    <div data-node-id="his.feak" data-node-label="Narrative Para One" className="w-fit text-[#1c1c1e] font-sans text-[13px] leading-[1.55] text-left block [overflow-wrap:normal]">已加载项目上下文。目标：把会话历史改为懒加载，避免启动时阻塞。接下来会检查 cache 与 history 相关模块。</div>
    <div data-node-id="his.caps" data-node-label="Narrative Para Two" className="w-fit text-[#3a3a3c] font-sans text-[13px] leading-[1.55] text-left block [overflow-wrap:normal]">agentOptions.provider/model 已切到编码预设；includeRuntimeContext: true；会话压缩标记 cpa-resp。</div>
  </div>
  <div data-node-id="his.gulp" data-node-label="Tool Row Bash" className="flex h-[30px] items-center bg-[#f0f0f2] border-t border-b border-l border-r rounded-lg px-2.5 py-0 gap-2 border-[#e8e8ec]">
    <div data-node-id="his.morg" data-node-label="SVG" className="size-[13px] block shrink-0 text-[#636366]">
      <svg stroke="rgb(99, 99, 102)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(99, 99, 102)" width="100%" height="100%"><polyline points="4 17 10 11 4 5" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></polyline><line x1="12" y1="19" x2="20" y2="19" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></line></svg>
    </div>
    <div data-node-id="his.cnet" data-node-label="Tool Row Bash Text" className="w-fit min-w-0 min-h-0 text-[#3a3a3c] font-mono text-xs leading-normal text-left [white-space-collapse:collapse] [text-wrap-mode:nowrap] [text-overflow:ellipsis] block [overflow-wrap:normal] overflow-clip">已运行 set -euo pipefail · cargo test -p nova-gateway</div>
  </div>
  <div data-node-id="his.they" data-node-label="Tool Row Read" className="flex h-[30px] items-center bg-[#f0f0f2] border-t border-b border-l border-r rounded-lg px-2.5 py-0 gap-2 border-[#e8e8ec]">
    <div data-node-id="his.nias" data-node-label="SVG" className="size-[13px] block shrink-0 text-[#636366]">
      <svg stroke="rgb(99, 99, 102)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(99, 99, 102)" width="100%" height="100%"><path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path><polyline points="13 2 13 9 20 9" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></polyline></svg>
    </div>
    <div data-node-id="his.kate" data-node-label="Tool Row Read Text" className="w-fit min-w-0 min-h-0 text-[#3a3a3c] font-mono text-xs leading-normal text-left [white-space-collapse:collapse] [text-wrap-mode:nowrap] [text-overflow:ellipsis] block [overflow-wrap:normal] overflow-clip">read_file · crates/nova-gateway/src/session/cache.rs</div>
  </div>
  <div data-node-id="his.hasp" data-node-label="Finding Inline" className="flex h-fit flex-col bg-[#fff7ed] border-t border-b border-l border-r rounded-[10px] px-3 py-2.5 gap-1.5 border-[#fed7aa]">
    <div data-node-id="his.knit" data-node-label="Finding Header" className="flex items-center gap-1.5">
      <div data-node-id="his.coto" data-node-label="SVG" className="size-[13px] block shrink-0 text-[#c2410c]">
        <svg stroke="rgb(194, 65, 12)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(194, 65, 12)" width="100%" height="100%"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" color="rgb(194, 65, 12)" fill="none" stroke="rgb(194, 65, 12)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path></svg>
      </div>
      <div data-node-id="his.acts" data-node-label="Note Title" className="w-fit text-[#1c1c1e] font-sans text-[12.5px] font-semibold leading-normal text-left block [overflow-wrap:normal]">提示 · 冷启动未命中缓存会阻塞</div>
    </div>
    <div data-node-id="his.roed" data-node-label="Note Snippet" className="w-fit text-[#636366] font-mono text-[11px] leading-[1.4] text-left block [overflow-wrap:normal]">cache.rs:48 · if !warmed.load(Ordering::Relaxed) &#123; block_on(load()); &#125;</div>
  </div>
  <div data-node-id="his.news" data-node-label="Patch Diff Card" className="flex h-fit flex-col bg-[#f5f5f7] border-t border-b border-l border-r rounded-[10px] px-3 py-2.5 gap-1 border-[#e5e5ea]">
    <div data-node-id="his.such" data-node-label="Patch Diff Header" className="flex items-center mt-0 mb-1 mx-0 gap-1.5">
      <div data-node-id="his.karo" data-node-label="SVG" className="size-[13px] block text-[#636366]">
        <svg stroke="rgb(99, 99, 102)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(99, 99, 102)" width="100%" height="100%"><polyline points="16 18 22 12 16 6" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></polyline><polyline points="8 6 2 12 8 18" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></polyline></svg>
      </div>
      <div data-node-id="his.form" data-node-label="Patch Diff Path" className="w-fit text-[#636366] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">cache.rs</div>
      <div data-node-id="his.year" data-node-label="Patch Diff Plus" className="w-fit text-[#16a34a] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">+12</div>
      <div data-node-id="his.duns" data-node-label="Patch Diff Minus" className="w-fit text-[#dc2626] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">-4</div>
    </div>
    <div data-node-id="his.gain" data-node-label="Diff Line Del" className="w-fit text-[#b91c1c] font-mono text-[11.5px] leading-[1.4] text-left block [overflow-wrap:normal]">- if !warmed.load(Ordering::Relaxed) &#123; block_on(load()); &#125;</div>
    <div data-node-id="his.quin" data-node-label="Diff Line Add" className="w-fit text-[#15803d] font-mono text-[11.5px] leading-[1.4] text-left block [overflow-wrap:normal]">+ spawn(async &#123; warm_cache().await &#125;);</div>
  </div>
  <div data-node-id="his.call" data-node-label="Canvas Feed Chip" className="flex h-[34px] items-center bg-white border-t border-b border-l border-r shadow-sm rounded-[9px] px-2.5 py-0 gap-2 border-[#e5e5ea]">
    <div data-node-id="his.xema" data-node-label="SVG" className="size-[13px] block shrink-0 text-[#0a84ff]">
      <svg stroke="rgb(10, 132, 255)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(10, 132, 255)" width="100%" height="100%"><rect x="3" y="3" width="18" height="18" rx="2" ry="2" color="rgb(10, 132, 255)" fill="none" stroke="rgb(10, 132, 255)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></rect><line x1="3" y1="9" x2="21" y2="9" color="rgb(10, 132, 255)" fill="none" stroke="rgb(10, 132, 255)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></line><line x1="9" y1="21" x2="9" y2="9" color="rgb(10, 132, 255)" fill="none" stroke="rgb(10, 132, 255)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></line></svg>
    </div>
    <div data-node-id="his.blog" data-node-label="Canvas Feed Chip Text" className="w-fit min-w-0 min-h-0 text-[#1c1c1e] font-sans text-xs font-medium leading-normal text-left [white-space-collapse:collapse] [text-wrap-mode:nowrap] [text-overflow:ellipsis] block [overflow-wrap:normal] flex-1 overflow-clip">测试覆盖看板 · 已生成</div>
    <div data-node-id="his.what" data-node-label="Canvas Open Link" className="w-fit shrink-0 text-[#0a84ff] font-sans text-[11px] font-medium leading-normal text-left block [overflow-wrap:normal]">打开</div>
  </div>
  <div data-node-id="his.grow" data-node-label="Context Compressed Note" className="flex h-fit items-center gap-1.5">
    <div data-node-id="his.blup" data-node-label="SVG" className="size-3 block text-[#8e8e93]">
      <svg stroke="rgb(142, 142, 147)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(142, 142, 147)" width="100%" height="100%"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" color="rgb(142, 142, 147)" fill="none" stroke="rgb(142, 142, 147)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path><polyline points="22 4 12 14.01 9 11.01" color="rgb(142, 142, 147)" fill="none" stroke="rgb(142, 142, 147)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></polyline></svg>
    </div>
    <div data-node-id="his.cole" data-node-label="Context Compressed Text" className="w-fit text-[#8e8e93] font-sans text-[11.5px] leading-normal text-left block [overflow-wrap:normal]">上下文已自动压缩</div>
  </div>
</div>

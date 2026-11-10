<div data-node-id="aal" data-node-label="Side Note Card Collapsed" data-component-x="23208" data-component-y="2616" className="flex w-[868px] h-fit flex-col pt-4 pb-6 bg-[#fafafa] font-sans [line-height:normal] px-8 gap-3 overflow-clip">
  <div data-node-id="aal.cue" data-node-label="User Turn" className="flex flex-col items-end pr-0 pl-[150px] py-0 gap-1.5">
    <div data-node-id="aal.you" data-node-label="User Bubble" className="bg-[#f3f3f5] text-[#1c1c1e] font-sans text-[15px] leading-[1.65] text-left block [overflow-wrap:normal] rounded-[20px] px-4 py-2.5">先别动 login.ts，把重试逻辑的问题列出来</div>
  </div>
  <div data-node-id="aal.ust" data-node-label="Assistant Reply" className="w-fit text-[#1c1c1e] font-sans text-[15px] leading-[1.75] text-left block [overflow-wrap:normal]">好的，我先把重试相关的调用链整理一下，再给出改动建议。</div>
  <div data-node-id="aal.but" data-node-label="Side Note Card" className="flex w-full flex-col bg-white border-t border-b border-l border-r rounded-[10px] overflow-clip border-[#e5e5ea]">
    <div data-node-id="aal.hut" data-node-label="Note Card Header" className="flex h-fit items-center px-3 py-2.5 gap-2">
      <div data-node-id="aal.far" data-node-label="SVG" className="size-3.5 block shrink-0 text-[#636366]">
        <svg stroke="rgb(99, 99, 102)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(99, 99, 102)" width="100%" height="100%"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path></svg>
      </div>
      <div data-node-id="aal.nul" data-node-label="Note Card Title" className="w-fit shrink-0 text-[#1c1c1e] font-sans text-[13px] font-medium leading-normal text-left block [overflow-wrap:normal]">来自侧边聊天的笔记</div>
      <div data-node-id="aal.are" data-node-label="Note Card Summary" className="w-fit min-w-0 min-h-0 text-[#8e8e93] font-sans text-[13px] leading-normal text-left [white-space-collapse:collapse] [text-wrap-mode:nowrap] [text-overflow:ellipsis] block [overflow-wrap:normal] flex-1 overflow-clip">login.ts 的 retry 里 sleep 300ms 是给 refreshToken() 留的退避，不是限流。</div>
      <div data-node-id="aal.yan" data-node-label="SVG" className="size-3.5 block shrink-0 text-[#8e8e93]">
        <svg stroke="rgb(142, 142, 147)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(142, 142, 147)" width="100%" height="100%"><path d="m6 9 6 6 6-6" color="rgb(142, 142, 147)" fill="none" stroke="rgb(142, 142, 147)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path></svg>
      </div>
    </div>
  </div>
  <div data-node-id="aal.jef" data-node-label="State Note" className="w-full bg-[#fff8e6] border-t border-b border-l border-r text-[#a05a00] font-sans text-[11px] leading-normal text-left block [overflow-wrap:normal] rounded-lg px-3 py-2 border-[#ffe1a6]">注释：「写进主线」确认后，主会话在写入位置插入这张笔记卡，默认收起：图标 + 标题 + 灰字摘要（笔记第一行，单行截断）+ 展开箭头。整行可点开；卡片本身只读，不能在主线里编辑。</div>
</div>

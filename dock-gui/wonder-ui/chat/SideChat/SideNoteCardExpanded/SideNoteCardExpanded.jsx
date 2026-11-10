<div data-node-id="two" data-node-label="Side Note Card Expanded" data-component-x="24136" data-component-y="2616" className="flex w-[868px] h-fit flex-col pt-4 pb-6 bg-[#fafafa] font-sans [line-height:normal] px-8 gap-3 overflow-clip">
  <div data-node-id="two.got" data-node-label="User Turn" className="flex flex-col items-end pr-0 pl-[150px] py-0 gap-1.5">
    <div data-node-id="two.vol" data-node-label="User Bubble" className="bg-[#f3f3f5] text-[#1c1c1e] font-sans text-[15px] leading-[1.65] text-left block [overflow-wrap:normal] rounded-[20px] px-4 py-2.5">先别动 login.ts，把重试逻辑的问题列出来</div>
  </div>
  <div data-node-id="two.beg" data-node-label="Assistant Reply" className="w-fit text-[#1c1c1e] font-sans text-[15px] leading-[1.75] text-left block [overflow-wrap:normal]">好的，我先把重试相关的调用链整理一下，再给出改动建议。</div>
  <div data-node-id="two.gmt" data-node-label="Side Note Card" className="flex w-full flex-col bg-white border-t border-b border-l border-r rounded-[10px] overflow-clip border-[#e5e5ea]">
    <div data-node-id="two.icq" data-node-label="Note Card Header" className="flex h-fit items-center px-3 py-2.5 gap-2">
      <div data-node-id="two.tew" data-node-label="SVG" className="size-3.5 block shrink-0 text-[#636366]">
        <svg stroke="rgb(99, 99, 102)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(99, 99, 102)" width="100%" height="100%"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" color="rgb(99, 99, 102)" fill="none" stroke="rgb(99, 99, 102)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path></svg>
      </div>
      <div data-node-id="two.ore" data-node-label="Note Card Title" className="w-fit shrink-0 text-[#1c1c1e] font-sans text-[13px] font-medium leading-normal text-left block [overflow-wrap:normal]">来自侧边聊天的笔记</div>
      <div data-node-id="two.art" data-node-label="Note Card Spacer" className="block flex-1"></div>
      <div data-node-id="two.max" data-node-label="SVG" className="size-3.5 block shrink-0 text-[#8e8e93]">
        <svg stroke="rgb(142, 142, 147)" fill="none" strokeWidth="2" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" color="rgb(142, 142, 147)" width="100%" height="100%"><path d="m18 15-6-6-6 6" color="rgb(142, 142, 147)" fill="none" stroke="rgb(142, 142, 147)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2px"></path></svg>
      </div>
    </div>
    <div data-node-id="two.elm" data-node-label="Note Card Body" className="flex flex-col border-t px-3 py-2.5 gap-1.5 border-t-[#ececf0]">
      <div data-node-id="two.you" data-node-label="Note Bullet One" className="flex text-[#1c1c1e] font-sans text-sm leading-[1.65] gap-2">
        <div data-node-id="two.was" data-node-label="Bullet Dot" className="w-fit shrink-0 text-[#8e8e93] font-sans text-sm leading-[1.65] text-left block [overflow-wrap:normal]">•</div>
        <div data-node-id="two.jar" data-node-label="Bullet Text" className="block">
          <span data-node-id="two.sum" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-sm leading-[1.65] text-left [overflow-wrap:normal]">login.ts 的 retry 里 sleep 300ms 是给 </span><span data-node-id="two.usa" data-node-label="Inline Code" className="inline bg-[#f0f0f2] text-[#3a3a3c] font-mono text-[12.5px] leading-[1.65] text-left [overflow-wrap:normal] rounded-md px-1.5 py-0.5">refreshToken()</span><span data-node-id="two.pot" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-sm leading-[1.65] text-left [overflow-wrap:normal]"> 留的退避，不是限流。</span>
        </div>
      </div>
      <div data-node-id="two.res" data-node-label="Note Bullet Two" className="flex text-[#1c1c1e] font-sans text-sm leading-[1.65] gap-2">
        <div data-node-id="two.rix" data-node-label="Bullet Dot" className="w-fit shrink-0 text-[#8e8e93] font-sans text-sm leading-[1.65] text-left block [overflow-wrap:normal]">•</div>
        <div data-node-id="two.can" data-node-label="Bullet Text" className="w-fit text-[#1c1c1e] font-sans text-sm leading-[1.65] text-left block [overflow-wrap:normal]">最多重试 3 次，总等待约 0.9s；改成 200 / 400 / 800ms 指数退避会到 1.4s。</div>
      </div>
      <div data-node-id="two.are" data-node-label="Note Bullet Three" className="flex text-[#1c1c1e] font-sans text-sm leading-[1.65] gap-2">
        <div data-node-id="two.cry" data-node-label="Bullet Dot" className="w-fit shrink-0 text-[#8e8e93] font-sans text-sm leading-[1.65] text-left block [overflow-wrap:normal]">•</div>
        <div data-node-id="two.cfr" data-node-label="Bullet Text" className="block">
          <span data-node-id="two.des" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-sm leading-[1.65] text-left [overflow-wrap:normal]">建议保留 3 次上限，改指数退避并加随机抖动；单测补连续 </span><span data-node-id="two.usr" data-node-label="Inline Code" className="inline bg-[#f0f0f2] text-[#3a3a3c] font-mono text-[12.5px] leading-[1.65] text-left [overflow-wrap:normal] rounded-md px-1.5 py-0.5">401</span><span data-node-id="two.geo" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-sm leading-[1.65] text-left [overflow-wrap:normal]"> 的用例。</span>
        </div>
      </div>
    </div>
    <div data-node-id="two.sur" data-node-label="Note Card Footer" className="pt-0 pb-2.5 text-[#8e8e93] font-sans text-[11px] leading-normal text-left block [overflow-wrap:normal] px-3">写入于 14:32 · 模型已看到</div>
  </div>
  <div data-node-id="two.way" data-node-label="State Note" className="w-full bg-[#fff8e6] border-t border-b border-l border-r text-[#a05a00] font-sans text-[11px] leading-normal text-left block [overflow-wrap:normal] rounded-lg px-3 py-2 border-[#ffe1a6]">注释：点标题行展开 / 收起，高度过渡约 200ms（ease-out），箭头同步旋转；展开后显示整段笔记（markdown 渲染），底部灰字写入时间 +「模型已看到」。</div>
</div>

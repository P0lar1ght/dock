<div data-node-id="rfc" data-node-label="Desktop Agent Cursor States" data-component-x="8716" data-component-y="4905" className="flex w-[1440px] h-fit min-h-[900px] flex-col bg-[#f7f7f8] [line-height:normal] px-12 py-10 gap-6 overflow-clip">
  <div data-node-id="rfc.fig" data-node-label="Header" className="flex flex-col gap-1">
    <div data-node-id="rfc.mot" data-node-label="Title" className="w-fit text-[#1c1c1e] font-sans text-xl font-semibold leading-normal text-left block [overflow-wrap:normal]">桌面实时画面 · Agent 光标</div>
    <div data-node-id="rfc.med" data-node-label="Subtitle" className="w-fit text-[#8e8e93] font-sans text-[13px] leading-normal text-left block [overflow-wrap:normal]">光标画在实时画面上（窗口截图里没有它）：位置来自 cua-driver 的 agent 光标，动作类型来自 Dock 的工具调用。蓝色 = agent，和你的鼠标区分开。</div>
  </div>
  <div data-node-id="rfc.wan" data-node-label="Action Grid" className="grid grid-cols-[minmax(0px,_1fr)_minmax(0px,_1fr)_minmax(0px,_1fr)_minmax(0px,_1fr)] gap-4">
    <div data-node-id="rfc.sow" data-node-label="Tile Move" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.woe" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.aby" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-3 gap-2">
          <div data-node-id="rfc.has" data-node-label="Bar" className="block w-24 h-2 bg-[#e5e5ea] rounded-full"></div>
          <div data-node-id="rfc.min" data-node-label="Bar" className="block w-[160px] h-2 bg-[#f2f2f4] rounded-full"></div>
          <div data-node-id="rfc.max" data-node-label="Bar" className="block w-[128px] h-2 bg-[#f2f2f4] rounded-full"></div>
        </div>
        <div data-node-id="rfc.you" data-node-label="Trail" className="inset-0 block absolute">
          <div data-node-id="rfc.cds" data-node-label="SVG" className="block w-[292px] h-[140px]">
            <svg viewBox="0 0 292 140" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M44 112 Q 70 40 150 64" stroke="#0a84ff" strokeOpacity="0.5" strokeWidth="2" strokeDasharray="3 6" strokeLinecap="round" fill="none"></path><circle cx="44" cy="112" r="3" fill="#0a84ff" fillOpacity="0.4"></circle></svg>
          </div>
        </div>
        <div data-node-id="rfc.tst" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[146px] top-[60px] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.rep" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.dsl" data-node-label="Cursor Tag" className="flex absolute items-center left-[168px] top-[86px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.jvc" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.jag" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 移动</div>
        </div>
      </div>
      <div data-node-id="rfc.yad" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.von" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">移动</div>
        <div data-node-id="rfc.pow" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">move_cursor</div>
      </div>
      <div data-node-id="rfc.yun" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">弹簧滑行约 420ms，走弧线 · 轨迹到点后 300ms 淡出 · 新目标打断时从当前位置接着滑</div>
    </div>
    <div data-node-id="rfc.mev" data-node-label="Tile Click" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.buy" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.via" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-3 gap-2">
          <div data-node-id="rfc.toy" data-node-label="Bar" className="block w-24 h-2 bg-[#e5e5ea] rounded-full"></div>
          <div data-node-id="rfc.que" data-node-label="Button" className="block w-20 h-7 mt-4 mr-0 mb-0 ml-[88px] bg-[#f2f2f4] border-t border-b border-l border-r rounded-md border-[#e5e5ea]"></div>
        </div>
        <div data-node-id="rfc.lad" data-node-label="Ripple Outer" className="size-12 block absolute left-[126px] top-10 rounded-full border-2 border-[#0a84ff59]"></div>
        <div data-node-id="rfc.may" data-node-label="Ripple Inner" className="size-6 block absolute left-[138px] top-[52px] bg-[#0a84ff38] rounded-full"></div>
        <div data-node-id="rfc.ast" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[146px] top-[60px] scale-90 [contain:layout] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px]">
          <div data-node-id="rfc.dis" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.con" data-node-label="Cursor Tag" className="flex absolute items-center left-[168px] top-[86px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.nig" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.hwy" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 点击</div>
        </div>
      </div>
      <div data-node-id="rfc.vol" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.bye" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">点击</div>
        <div data-node-id="rfc.tim" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">click</div>
      </div>
      <div data-node-id="rfc.our" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">先滑到点 · 按下时光标缩到 0.9 再回弹 120ms · 涟漪从光标尖端扩散，480ms 淡出</div>
    </div>
    <div data-node-id="rfc.ova" data-node-label="Tile Double Click" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.cst" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.age" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-3 gap-2">
          <div data-node-id="rfc.web" data-node-label="Bar" className="block w-24 h-2 bg-[#e5e5ea] rounded-full"></div>
          <div data-node-id="rfc.san" data-node-label="File Icon" className="block w-8 h-10 mt-3 mr-0 mb-0 ml-[104px] bg-[#f2f2f4] border-t border-b border-l border-r rounded-[3px] border-[#e5e5ea]"></div>
        </div>
        <div data-node-id="rfc.die" data-node-label="Ripple Far" className="size-16 block absolute left-[118px] top-8 rounded-full border-2 border-[#0a84ff2e]"></div>
        <div data-node-id="rfc.par" data-node-label="Ripple Near" className="size-11 block absolute left-[128px] top-[42px] rounded-full border-2 border-[#0a84ff66]"></div>
        <div data-node-id="rfc.are" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[146px] top-[60px] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.bab" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.awa" data-node-label="Cursor Tag" className="flex absolute items-center left-[168px] top-[86px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.kra" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.ace" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 双击 ×2</div>
        </div>
      </div>
      <div data-node-id="rfc.tom" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.cry" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">双击</div>
        <div data-node-id="rfc.soh" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">double_click</div>
      </div>
      <div data-node-id="rfc.gmt" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">两圈涟漪间隔 120ms · 光标连按两下 · 标签带 ×2</div>
    </div>
    <div data-node-id="rfc.aid" data-node-label="Tile Right Click" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.cvs" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.pun" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-3 gap-2">
          <div data-node-id="rfc.lek" data-node-label="Bar" className="block w-24 h-2 bg-[#e5e5ea] rounded-full"></div>
          <div data-node-id="rfc.llp" data-node-label="Bar" className="block w-[160px] h-2 bg-[#f2f2f4] rounded-full"></div>
        </div>
        <div data-node-id="rfc.cel" data-node-label="Ripple Dashed" className="size-12 block absolute left-[126px] top-10 border-dashed rounded-full border-2 border-[#0a84ff73]"></div>
        <div data-node-id="rfc.mhz" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[146px] top-[60px] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.mou" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.tew" data-node-label="Cursor Tag" className="flex absolute items-center left-[168px] top-[86px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.zar" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.dig" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 右键</div>
        </div>
      </div>
      <div data-node-id="rfc.pst" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.ide" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">右键</div>
        <div data-node-id="rfc.him" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">right_click</div>
      </div>
      <div data-node-id="rfc.kon" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">涟漪换成虚线圈 · 不画菜单：真实的右键菜单下一帧就出现在画面里</div>
    </div>
    <div data-node-id="rfc.was" data-node-label="Tile Drag" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.yet" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.rix" data-node-label="Mini Window" className="inset-4 flex absolute items-start bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-4 gap-6">
          <div data-node-id="rfc.way" data-node-label="Source File" className="block w-8 h-10 bg-[#e8f1ff] border-t border-b border-l border-r rounded-[3px] border-[#b9d6ff]"></div>
          <div data-node-id="rfc.sas" data-node-label="Target Folder" className="block w-11 h-9 mt-1 mr-0 mb-0 ml-[100px] bg-[#66adff] shadow-[0px_0px_0px_2px_rgb(255,255,255),0px_0px_0px_4px_rgb(10,132,255)] rounded-md"></div>
        </div>
        <div data-node-id="rfc.oct" data-node-label="Trail" className="inset-0 block absolute">
          <div data-node-id="rfc.jul" data-node-label="SVG" className="block w-[292px] h-[140px]">
            <svg viewBox="0 0 292 140" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M48 52 Q 110 6 176 48" stroke="#0a84ff" strokeOpacity="0.5" strokeWidth="2" strokeDasharray="3 6" strokeLinecap="round" fill="none"></path><circle cx="48" cy="52" r="3.5" fill="#0a84ff" fillOpacity="0.45"></circle></svg>
          </div>
        </div>
        <div data-node-id="rfc.sms" data-node-label="Drag Ghost" className="block absolute w-7 h-9 left-[188px] top-[62px] rotate-[-6deg] [contain:layout] bg-white border-t border-b border-l border-r shadow-[0px_6px_14px_rgba(0,0,0,0.18)] rounded-[3px] border-[#d1d1d6]"></div>
        <div data-node-id="rfc.sha" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[172px] top-11 drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.one" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.won" data-node-label="Cursor Tag" className="flex absolute items-center left-[206px] top-[104px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.can" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.ray" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 拖动</div>
        </div>
      </div>
      <div data-node-id="rfc.fad" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.com" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">拖动</div>
        <div data-node-id="rfc.lot" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">drag</div>
      </div>
      <div data-node-id="rfc.bey" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">起点留一个点 · 光标挂着影子沿弧线走 · 松手时影子缩进落点 200ms</div>
    </div>
    <div data-node-id="rfc.bid" data-node-label="Tile Type" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.loc" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.out" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-4 gap-3">
          <div data-node-id="rfc.see" data-node-label="Bar" className="block w-20 h-2 bg-[#e5e5ea] rounded-full"></div>
          <div data-node-id="rfc.yon" data-node-label="Text Field" className="flex w-[176px] h-8 items-center rounded-md px-2 py-0 gap-0.5 border-2 border-[#0a84ff]">
            <div data-node-id="rfc.map" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-xs leading-none text-left block [overflow-wrap:normal]">10月</div>
            <div data-node-id="rfc.any" data-node-label="Caret" className="block w-[1.5px] h-4 bg-[#0a84ff]"></div>
          </div>
        </div>
        <div data-node-id="rfc.sep" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[206px] top-[60px] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.coe" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.luo" data-node-label="Cursor Tag" className="flex absolute items-center left-[150px] top-[100px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.nba" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.css" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 输入「10月」</div>
        </div>
      </div>
      <div data-node-id="rfc.pay" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.sew" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">输入</div>
        <div data-node-id="rfc.pic" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">type_text · set_value</div>
      </div>
      <div data-node-id="rfc.kid" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">光标停在输入位置旁 · 标签带正在打的字（最多 12 字，多了截断）</div>
    </div>
    <div data-node-id="rfc.sci" data-node-label="Tile Scroll" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.wiz" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.war" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-3 gap-2">
          <div data-node-id="rfc.sak" data-node-label="Bar" className="block w-[160px] h-2 bg-[#f2f2f4] rounded-full"></div>
          <div data-node-id="rfc.oam" data-node-label="Bar" className="block w-[144px] h-2 bg-[#f2f2f4] rounded-full"></div>
          <div data-node-id="rfc.rut" data-node-label="Bar" className="block w-[176px] h-2 bg-[#f2f2f4] rounded-full"></div>
          <div data-node-id="rfc.lan" data-node-label="Bar" className="block w-[128px] h-2 bg-[#f2f2f4] rounded-full"></div>
        </div>
        <div data-node-id="rfc.due" data-node-label="Scroll Badge" className="size-7 flex absolute flex-col justify-center items-center left-[118px] top-[50px] bg-white border-t border-b border-l border-r shadow-[0px_4px_10px_rgba(0,0,0,0.12)] rounded-full border-[#e5e5ea]">
          <div data-node-id="rfc.ash" data-node-label="SVG" className="size-3.5 block">
            <svg viewBox="0 0 24 24" fill="none" stroke="#0a84ff" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><polyline points="6 9 12 15 18 9" fill="none" stroke="rgb(10, 132, 255)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2.4px"></polyline></svg>
          </div>
        </div>
        <div data-node-id="rfc.nit" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[146px] top-[60px] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.bio" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.chi" data-node-label="Cursor Tag" className="flex absolute items-center left-[168px] top-[86px] pr-2 pl-1.5 bg-[#0a84ff] rounded-full py-0.5 gap-1">
          <div data-node-id="rfc.apr" data-node-label="Text" className="w-fit text-white font-sans text-[10px] font-semibold leading-normal text-left block [overflow-wrap:normal]">Agent</div>
          <div data-node-id="rfc.thu" data-node-label="Text" className="w-fit text-[#ffffffcc] font-sans text-[10px] leading-normal text-left block [overflow-wrap:normal]">· 向下滚动</div>
        </div>
      </div>
      <div data-node-id="rfc.uca" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.ice" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">滚动</div>
        <div data-node-id="rfc.sky" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">scroll</div>
      </div>
      <div data-node-id="rfc.gif" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">光标不动 · 方向角标往滚动方向推 6px 再回，400ms</div>
    </div>
    <div data-node-id="rfc.but" data-node-label="Tile Key" className="flex flex-col bg-white border-t border-b border-l border-r rounded-xl p-4 gap-3 border-[#e5e5ea]">
      <div data-node-id="rfc.elb" data-node-label="Preview" className="block relative h-[140px] bg-[#e4e6ea] rounded-[10px] overflow-clip">
        <div data-node-id="rfc.hrs" data-node-label="Mini Window" className="inset-4 flex absolute flex-col bg-white shadow-[0px_4px_14px_rgba(0,0,0,0.1)] rounded-lg p-3 gap-2">
          <div data-node-id="rfc.his" data-node-label="Bar" className="block w-24 h-2 bg-[#e5e5ea] rounded-full"></div>
          <div data-node-id="rfc.who" data-node-label="Bar" className="block w-[160px] h-2 bg-[#f2f2f4] rounded-full"></div>
        </div>
        <div data-node-id="rfc.fam" data-node-label="Cursor Arrow" className="size-[22px] block absolute left-[146px] top-[60px] drop-shadow-[rgba(0,_0,_0,_0.3)_0px_2px_4px] [contain:layout]">
          <div data-node-id="rfc.wer" data-node-label="SVG" className="size-[22px] block">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><path d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z" fill="#0a84ff" stroke="#ffffff" strokeWidth="1.6" strokeLinejoin="round"></path></svg>
          </div>
        </div>
        <div data-node-id="rfc.bud" data-node-label="Keycaps" className="flex absolute items-center left-[100px] top-[90px] gap-1">
          <div data-node-id="rfc.men" data-node-label="Keycap" className="min-w-6 h-6 content-center bg-[#1c1c1e] shadow-[0px_2px_0px_rgb(0,0,0)] text-white font-sans text-xs font-medium leading-[1.33333] text-center block [overflow-wrap:normal] rounded-md px-1.5 py-0">⌘</div>
          <div data-node-id="rfc.etc" data-node-label="Keycap" className="min-w-6 h-6 content-center bg-[#1c1c1e] shadow-[0px_2px_0px_rgb(0,0,0)] text-white font-sans text-xs font-medium leading-[1.33333] text-center block [overflow-wrap:normal] rounded-md px-1.5 py-0">⇧</div>
          <div data-node-id="rfc.fub" data-node-label="Keycap" className="min-w-6 h-6 content-center bg-[#1c1c1e] shadow-[0px_2px_0px_rgb(0,0,0)] text-white font-sans text-xs font-medium leading-[1.33333] text-center block [overflow-wrap:normal] rounded-md px-1.5 py-0">N</div>
        </div>
      </div>
      <div data-node-id="rfc.cab" data-node-label="Label Row" className="flex items-baseline gap-2">
        <div data-node-id="rfc.tax" data-node-label="Text" className="w-fit text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left block [overflow-wrap:normal]">按键</div>
        <div data-node-id="rfc.iao" data-node-label="Text" className="w-fit text-[#8e8e93] font-mono text-[11px] leading-normal text-left block [overflow-wrap:normal]">press_key · hotkey</div>
      </div>
      <div data-node-id="rfc.pop" data-node-label="Spec" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">不移动光标（变淡）· 键帽在光标下方弹出，900ms 后淡出</div>
    </div>
  </div>
  <div data-node-id="rfc.six" data-node-label="Live Status Row" className="flex gap-4">
    <div data-node-id="rfc.daw" data-node-label="Status No Window" className="flex flex-col bg-white border-t border-b border-l border-r flex-1 rounded-xl p-4 gap-2.5 border-[#e5e5ea]">
      <div data-node-id="rfc.lye" data-node-label="Header Strip" className="flex items-center bg-[#f2f2f4] rounded-md px-3 py-2 gap-2">
        <div data-node-id="rfc.rec" data-node-label="Dot" className="size-2 block shrink-0 bg-[#aeaeb2] rounded-full"></div>
        <div data-node-id="rfc.cbs" data-node-label="Text" className="w-fit text-[#3a3a3c] font-sans text-xs font-medium leading-normal text-left block [overflow-wrap:normal]">实时画面</div>
        <div data-node-id="rfc.ati" data-node-label="Text" className="w-fit text-[#8e8e93] font-sans text-xs leading-normal text-left block [overflow-wrap:normal]">没有可看的窗口 · agent 打开应用后自动跟上</div>
      </div>
      <div data-node-id="rfc.its" data-node-label="Text" className="block text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal">
        <span data-node-id="rfc.fen" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left [overflow-wrap:normal]">没有窗口</span><span data-node-id="rfc.dao" data-node-label="Text" className="inline mr-0 ml-2 text-[#8e8e93] font-mono text-[11px] leading-normal text-left [overflow-wrap:normal] my-0">no_window</span>
      </div>
      <div data-node-id="rfc.api" data-node-label="Text" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">agent 没碰过窗口，前台也只有 Dock 自己 / 系统界面。不转圈：画面区显示这句话，0.8 秒后自己再找。</div>
    </div>
    <div data-node-id="rfc.pox" data-node-label="Status Capture Failed" className="flex flex-col bg-white border-t border-b border-l border-r flex-1 rounded-xl p-4 gap-2.5 border-[#e5e5ea]">
      <div data-node-id="rfc.per" data-node-label="Header Strip" className="flex items-center bg-[#fff8eb] rounded-md px-3 py-2 gap-2">
        <div data-node-id="rfc.bob" data-node-label="Dot" className="size-2 block shrink-0 bg-[#ff9f0a] rounded-full"></div>
        <div data-node-id="rfc.sue" data-node-label="Text" className="w-fit text-[#3a3a3c] font-sans text-xs font-medium leading-normal text-left block [overflow-wrap:normal]">实时画面</div>
        <div data-node-id="rfc.uji" data-node-label="Text" className="w-fit text-[#b25e00] font-sans text-xs leading-normal text-left block [overflow-wrap:normal]">截不到「Grok Bot」：窗口已关闭 · 正在重试</div>
      </div>
      <div data-node-id="rfc.ibm" data-node-label="Text" className="block text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal">
        <span data-node-id="rfc.ahu" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left [overflow-wrap:normal]">截图失败</span><span data-node-id="rfc.jpg" data-node-label="Text" className="inline mr-0 ml-2 text-[#8e8e93] font-mono text-[11px] leading-normal text-left [overflow-wrap:normal] my-0">capture_failed</span>
      </div>
      <div data-node-id="rfc.ran" data-node-label="Text" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">带 cua-driver 给的原因。上一帧保留但变灰；连续失败就换回最前面的窗口。恢复后自动回到实时。</div>
    </div>
    <div data-node-id="rfc.aus" data-node-label="Status Driver Gone" className="flex flex-col bg-white border-t border-b border-l border-r flex-1 rounded-xl p-4 gap-2.5 border-[#e5e5ea]">
      <div data-node-id="rfc.yis" data-node-label="Header Strip" className="flex items-center bg-[#fff2f1] rounded-md px-3 py-2 gap-2">
        <div data-node-id="rfc.usd" data-node-label="Dot" className="size-2 block shrink-0 bg-[#ff3b30] rounded-full"></div>
        <div data-node-id="rfc.ade" data-node-label="Text" className="w-fit text-[#3a3a3c] font-sans text-xs font-medium leading-normal text-left block [overflow-wrap:normal]">实时画面</div>
        <div data-node-id="rfc.mac" data-node-label="Text" className="w-fit text-[#d70015] font-sans text-xs leading-normal text-left block [overflow-wrap:normal]">cua-driver 断开了，正在重连…</div>
      </div>
      <div data-node-id="rfc.hid" data-node-label="Text" className="block text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal">
        <span data-node-id="rfc.fro" data-node-label="Text" className="inline text-[#1c1c1e] font-sans text-[13px] font-semibold leading-normal text-left [overflow-wrap:normal]">连不上 cua-driver</span><span data-node-id="rfc.ail" data-node-label="Text" className="inline mr-0 ml-2 text-[#8e8e93] font-mono text-[11px] leading-normal text-left [overflow-wrap:normal] my-0">driver_unavailable</span>
      </div>
      <div data-node-id="rfc.hay" data-node-label="Text" className="w-fit text-[#636366] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">开画面这一步也有上限（5 秒），不会一直「正在连接」。光标隐藏，2 秒后重开。</div>
    </div>
  </div>
  <div data-node-id="rfc.ing" data-node-label="Footnote" className="w-fit text-[#8e8e93] font-sans text-[11px] leading-relaxed text-left block [overflow-wrap:normal]">空闲：agent 停手 3 秒后光标淡到 40%，20 秒后隐藏（跟 cua-driver 的 idle_hide 一致）· 回看截图时不画光标 · 遵守系统「减少动态效果」：只瞬移、不画轨迹和涟漪。</div>
</div>

// 沙箱 iframe 里的文档预览页（Word / PowerPoint）。文档是不可信内容，不进主窗口的 DOM：
//
// - iframe 只开 allow-scripts，不同源，碰不到 Tauri；
// - CSP：脚本只认带 nonce 的内联和 data: 库（文档里夹的 onerror 之类跑不了），不联网；
// - 外链不在 iframe 里打开，交给外面用浏览器开；⌘ 快捷键转给外面；
// - 页面比面板宽就整体缩小（zoom），窄面板也能看全一页。
//
// 页面 → 外面：ready / progress { pages } / rendered { pages, ms } / error / link / key。
// 外面 → 页面：{ type: "doc", data: ArrayBuffer }（transfer）。

/** 页面 → 外面。 */
export type FromFrame =
  | { type: "ready" }
  /** 已经画出来的页数（分批画时陆续来）。 */
  | { type: "progress"; pages: number }
  | { type: "rendered"; pages: number; ms: number }
  | { type: "error"; message: string }
  | { type: "link"; href: string }
  | { type: "key"; key: string; code: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean };

const libCache = new Map<string, Promise<string>>();

function libUrl(name: string, load: () => Promise<string>): Promise<string> {
  let p = libCache.get(name);
  if (!p) {
    p = load().then((src) => `data:text/javascript;base64,${toBase64(src)}`);
    libCache.set(name, p);
  }
  return p;
}

function toBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/** 两种页面共用：发消息、外链、快捷键、按宽度缩放（`fit(页宽)`）。 */
const RUNTIME = `
var P=window.parent,root=document.getElementById("root"),styles=document.getElementById("styles"),pageW=0;
function post(m){m.__dockFrame=1;try{P.postMessage(m,"*")}catch(e){}}
function fail(err){post({type:"error",message:String(err&&err.message||err)})}
function fit(w,grow){if(w)pageW=w;if(!pageW)return;var z=(window.innerWidth-32)/pageW;root.style.zoom=Math.max(.2,grow?Math.min(z,1.5):Math.min(z,1))}
window.addEventListener("resize",function(){fit()});
document.addEventListener("click",function(e){
  var a=e.target&&e.target.closest&&e.target.closest("a[href]");if(!a)return;
  var href=a.getAttribute("href")||"";if(href.charAt(0)==="#")return;
  e.preventDefault();if(/^(https?:|mailto:)/i.test(href))post({type:"link",href:href});
});
window.addEventListener("keydown",function(e){if(!(e.metaKey||e.ctrlKey)||/^[ac]$/i.test(e.key))return;e.preventDefault();post({type:"key",key:e.key,code:e.code,metaKey:e.metaKey,ctrlKey:e.ctrlKey,shiftKey:e.shiftKey,altKey:e.altKey})});
window.addEventListener("message",function(e){if(e.source!==P)return;var m=e.data;if(m&&m.type==="doc")render(m.data)});
`;

/**
 * 拼预览页：`libs` 是要先加载的库（`?raw` 源码，走 data: URL），`script` 定义
 * `render(arrayBuffer)`，画完调 `post({type:"rendered",...})`。
 */
export async function frameSrcdoc(opts: {
  libs: [name: string, load: () => Promise<string>][];
  style: string;
  script: string;
}): Promise<string> {
  const urls = await Promise.all(opts.libs.map(([name, load]) => libUrl(name, load)));
  const nonce = crypto.randomUUID().replace(/-/g, "");
  const csp = [
    "default-src 'none'",
    `script-src 'nonce-${nonce}' data:`,
    "style-src 'unsafe-inline'",
    "img-src data: blob:",
    "font-src data:",
    "media-src data: blob:",
  ].join("; ");
  const libs = urls.map((u) => `<script nonce="${nonce}" src="${u}"></script>`).join("");
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>${opts.style}</style>${libs}</head><body><div id="styles"></div><div id="root"></div><script nonce="${nonce}">${RUNTIME}${opts.script}\npost({type:"ready"});</script></body></html>`;
}

const BASE_STYLE = `html,body{margin:0;background:#f2f2f4;font-family:-apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif}`;

/** Word：docx-preview 排版；没写页面大小的按 A4、Word 默认页边距画。几百页也只排视口附近的。 */
export function docxSrcdoc(): Promise<string> {
  return frameSrcdoc({
    libs: [
      ["jszip", () => import("../../../node_modules/jszip/dist/jszip.min.js?raw").then((m) => m.default)],
      ["docx-preview", () => import("../../../node_modules/docx-preview/dist/docx-preview.min.js?raw").then((m) => m.default)],
    ],
    style: `${BASE_STYLE}
.docx-wrapper{background:#f2f2f4!important;padding:24px 16px!important;display:flex;flex-flow:column;align-items:center}
.docx-wrapper>section.docx{box-shadow:0 8px 24px rgba(0,0,0,.12)!important;margin-bottom:20px!important;content-visibility:auto;contain-intrinsic-size:auto 794px auto 1123px}
.docx-wrapper>section.docx:not([style*="width"]){width:794px;min-height:1123px;padding:96px 120px;box-sizing:border-box}`,
    script: `
function render(data){
  var t0=performance.now();
  window.docx.renderAsync(data,root,styles,{inWrapper:true,breakPages:true,ignoreLastRenderedPageBreak:true,useBase64URL:true,renderComments:false,renderChanges:false}).then(function(){
    var s=root.querySelector("section.docx");fit(s?s.offsetWidth+32:0);
    post({type:"rendered",pages:root.querySelectorAll("section.docx").length,ms:Math.round(performance.now()-t0)});
  },fail);
}`,
  });
}

/** 幻灯片排版的固定大小（16:9）：再按面板宽度缩放。4:3 的在框里加边。 */
const SLIDE_W = 960;
const SLIDE_H = 540;

/**
 * PowerPoint：@jvmr/pptx-to-html 把每页转成一段 HTML。插进来之前先清一遍（脚本、内嵌框、
 * on* 属性、javascript: 链接；CSP 之外再挡一层）。先画前 3 页让人看到，其余每帧补一批。
 */
export function pptxSrcdoc(): Promise<string> {
  return frameSrcdoc({
    libs: [["pptx-to-html", () => import("./vendor/pptx-frame.js?raw").then((m) => m.default)]],
    style: `${BASE_STYLE}
#root{padding:20px 16px;display:flex;flex-flow:column;align-items:center;gap:16px}
section.slide{width:${SLIDE_W}px;height:${SLIDE_H}px;position:relative;overflow:hidden;background:#fff;box-shadow:0 6px 20px rgba(0,0,0,.12);content-visibility:auto;contain-intrinsic-size:${SLIDE_W}px ${SLIDE_H}px}
section.slide::after{content:attr(data-n);position:absolute;right:8px;bottom:6px;font-size:11px;color:#c7c7cc}`,
    script: `
var BAD="script,iframe,object,embed,link,meta,base,form,frame,frameset";
function clean(html){
  var doc=new DOMParser().parseFromString(html,"text/html");
  doc.querySelectorAll(BAD).forEach(function(n){n.remove()});
  doc.querySelectorAll("*").forEach(function(n){
    for(var i=n.attributes.length-1;i>=0;i--){var a=n.attributes[i],v=(a.value||"").trim().toLowerCase();
      if(/^on/i.test(a.name)||((a.name==="href"||a.name==="src"||a.name==="xlink:href"||a.name==="action")&&v.indexOf("javascript:")===0))n.removeAttribute(a.name)}
  });
  return doc.body;
}
function render(data){
  var t0=performance.now();
  window.pptxToHtml(data,{width:${SLIDE_W},height:${SLIDE_H},scaleToFit:true,letterbox:true}).then(function(slides){
    var i=0;fit(${SLIDE_W + 32},true);
    function batch(){
      var end=Math.min(slides.length,i+(i===0?3:12));
      for(;i<end;i++){var s=document.createElement("section");s.className="slide";s.setAttribute("data-n",String(i+1));
        var body=clean(slides[i]);while(body.firstChild)s.appendChild(body.firstChild);root.appendChild(s)}
      if(i<slides.length){post({type:"progress",pages:i});requestAnimationFrame(batch)}
      else post({type:"rendered",pages:slides.length,ms:Math.round(performance.now()-t0)});
    }
    if(!slides.length)post({type:"rendered",pages:0,ms:0});else batch();
  },fail);
}`,
  });
}

// 图谱的画布引擎：力导向布局（d3-force）+ Canvas 2D 绘制 + 指针交互。
// 不进 React 的渲染循环：布局每一帧都在动，画布自己画；React 只把数据、选中状态和设置交进来。
//
// 坐标：世界坐标是布局算出来的；屏幕 = 世界 × k + (x, y)。
// 点的屏幕大小按 √k 跟着缩放（缩小看全貌时不至于小成一粒灰，放大时也不会大得盖住别的）；
// 连线粗细、文字字号不随缩放变。
// 标签随缩放出现：点在屏幕上够大才标（链接多的大点先标），悬停 / 选中的那篇和它的邻居总是标。
// 相似建议默认只画悬停 / 选中那篇的，也不参与布局（不然看不见的线把点拉成一团）。
// 整体围成一个圆：横竖拉力一样，没有链接的笔记散在外圈；多个项目时各簇也排在一个圆上。
//
// 同一个引擎也画抽屉里的局部图谱（`local`）：不记位置、不拖线、不点线，滚轮留给抽屉滚动，标签全标。

import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type ForceCollide,
  type ForceLink,
  type ForceManyBody,
  type ForceX,
  type ForceY,
  type Simulation,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";
import { DEFAULT_SETTINGS, type GraphSettings } from "./settings";
import { GROUP_COLOR, type ViewEdge, type ViewNode } from "./view";

interface SimNode extends SimulationNodeDatum {
  id: string;
  /** 布局半径（世界坐标）：按链接数定。画出来的大小另按缩放算，见 `screenR`。 */
  r: number;
  view: ViewNode;
  /** 所在 scope 的聚集点。 */
  ax: number;
  ay: number;
  x: number;
  y: number;
  /** 连着几条参与布局的线：枢纽上的每一条拉得轻些，不然枢纽被扯来扯去。 */
  pull: number;
}

interface SimLink extends SimulationLinkDatum<SimNode> {
  source: SimNode;
  target: SimNode;
  view: ViewEdge;
}

export interface GraphTip {
  /** 提示框左上角（画布内布局像素）。 */
  x: number;
  y: number;
  /** 右边放不下时，提示框改放在这条竖线左边。 */
  left: number;
  node?: ViewNode;
  edge?: { view: ViewEdge; source: ViewNode; target: ViewNode };
}

export interface GraphCallbacks {
  onSelect: (node: ViewNode | null) => void;
  /** ⌥ 拖动：从一篇拖到另一篇。 */
  onLink: (from: ViewNode, to: ViewNode) => void;
  onEdge: (edge: ViewEdge, source: ViewNode, target: ViewNode, at: { x: number; y: number }) => void;
  onTip: (tip: GraphTip | null) => void;
  onCursor: (cursor: string) => void;
}

interface ViewTransform {
  k: number;
  x: number;
  y: number;
}

type Drag =
  | { type: "node"; node: SimNode; moved: boolean; startX: number; startY: number }
  | { type: "pan"; moved: boolean; startX: number; startY: number; view: ViewTransform }
  | { type: "link"; from: SimNode; x: number; y: number; startX: number; startY: number };

const MIN_K = 0.1;
const MAX_K = 4;
const INK = "#1c1c1e";
const INK_2 = "#3a3a3c";
const INK_4 = "#8e8e93";
const LINE = "#d1d1d6";
const LINE_STRONG = "#636366";
const SUGGEST = "#8e8e93";
/** 按条件着色时没命中的点。 */
const MUTED = "#a1a1a6";
const SURFACE = "rgba(255,255,255,0.94)";
const FONT = '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", sans-serif';

/** 上次布局停下时各点的位置：重新打开这一页、数据重拉后，图不会重新洗牌。 */
const remembered = new Map<string, { x: number; y: number }>();

function hash(text: string): number {
  let h = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** 链接越多点越大，差距拉得开：一眼看得出哪几篇是枢纽。 */
const radiusOf = (degree: number, sizePct: number) => Math.min(26, 3.5 + 2.4 * Math.sqrt(degree)) * (sizePct / 100);

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

const reducedMotion = () => typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

export interface EngineOptions {
  /** 抽屉里的局部图谱。 */
  local?: boolean;
}

export class GraphEngine {
  private ctx: CanvasRenderingContext2D;
  private nodes: SimNode[] = [];
  private links: SimLink[] = [];
  private byId = new Map<string, SimNode>();
  private neighbours = new Map<string, Set<string>>();
  private sim: Simulation<SimNode, SimLink>;
  private view: ViewTransform = { k: 1, x: 0, y: 0 };
  private width = 0;
  private height = 0;
  private dpr = 1;
  private insetRight = 0;
  private hover: SimNode | null = null;
  private hoverEdge: SimLink | null = null;
  private selected: string | null = null;
  private highlight: Set<string> | null = null;
  private colors: Map<string, string> | null = null;
  private settings: GraphSettings = DEFAULT_SETTINGS;
  private scopeLabels = new Map<string, string>();
  private drag: Drag | null = null;
  private frame = 0;
  private fitted = false;
  /** 画面还是「适应窗口」时的样子（之后没平移、没缩放）：抽屉开合时重新适应，而不是只挪半个抽屉宽。 */
  private autoFit = false;
  private anim: { from: ViewTransform; to: ViewTransform; start: number; ms: number } | null = null;
  private gestureK = 1;
  private disposed = false;
  /** 数据大变（换了项目 / 范围）后等布局快稳了再适应一次窗口。 */
  private refit = false;
  /** 刚点过的那个点 / 那条线：详情已经在抽屉或卡片里了，指针挪开之前不再弹悬停提示（会挡住旁边的东西）。 */
  private quietTip: string | null = null;
  /** 指针在画布上的位置（布局像素）；不在画布上是 null。图在指针底下动（布局还在收、滚轮平移）时靠它重新判断悬停。 */
  private pointer: { x: number; y: number } | null = null;
  /** 上一帧画出来的标签（屏幕坐标）：指着标签也算指着那篇。 */
  private labelHits: { node: SimNode; x1: number; y1: number; x2: number; y2: number }[] = [];
  private labelCache = new Map<string, string>();
  private readonly local: boolean;

  constructor(
    private canvas: HTMLCanvasElement,
    private cb: GraphCallbacks,
    options: EngineOptions = {},
  ) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    this.ctx = ctx;
    this.local = !!options.local;
    this.sim = forceSimulation<SimNode, SimLink>([])
      .force("link", forceLink<SimNode, SimLink>([]).id((d) => d.id))
      .force("charge", forceManyBody<SimNode>().distanceMax(800))
      .force("collide", forceCollide<SimNode>())
      // 用往聚集点的拉力代替「居中」：没有链接的笔记不会被斥力推到天边，而是散在外面一圈。
      // 横竖拉力一样，整张图围成一个圆，好看全貌。
      .force("x", forceX<SimNode>((d) => d.ax))
      .force("y", forceY<SimNode>((d) => d.ay))
      .alphaDecay(0.04)
      .on("tick", () => {
        // 换了范围之后的那次重排：快停下来时再对一次画面，新来的那几簇才都在视野里。
        if (this.refit && this.sim.alpha() < 0.12) {
          this.refit = false;
          this.fit(true);
        }
        this.invalidate();
      })
      .on("end", () => this.remember())
      .stop();
    this.applyForces();

    canvas.addEventListener("pointerdown", this.onPointerDown);
    canvas.addEventListener("pointermove", this.onPointerMove);
    canvas.addEventListener("pointerup", this.onPointerUp);
    canvas.addEventListener("pointercancel", this.onPointerUp);
    canvas.addEventListener("pointerleave", this.onPointerLeave);
    canvas.addEventListener("dblclick", this.onDoubleClick);
    canvas.addEventListener("wheel", this.onWheel, { passive: false });
    // WKWebView（Tauri）里双指捏合是 gesture 事件，不是 ctrl + 滚轮。
    canvas.addEventListener("gesturestart", this.onGestureStart as EventListener);
    canvas.addEventListener("gesturechange", this.onGestureChange as EventListener);
  }

  destroy() {
    this.disposed = true;
    this.remember();
    this.sim.stop();
    cancelAnimationFrame(this.frame);
    const c = this.canvas;
    c.removeEventListener("pointerdown", this.onPointerDown);
    c.removeEventListener("pointermove", this.onPointerMove);
    c.removeEventListener("pointerup", this.onPointerUp);
    c.removeEventListener("pointercancel", this.onPointerUp);
    c.removeEventListener("pointerleave", this.onPointerLeave);
    c.removeEventListener("dblclick", this.onDoubleClick);
    c.removeEventListener("wheel", this.onWheel);
    c.removeEventListener("gesturestart", this.onGestureStart as EventListener);
    c.removeEventListener("gesturechange", this.onGestureChange as EventListener);
  }

  // ---- 数据 ----

  /** `scopeLabels`：scope id → 项目名（同时有三个以上 scope 时标在各自那一簇上面）。 */
  setData(nodes: ViewNode[], edges: ViewEdge[], scopeLabels: Map<string, string>, currentScope: string | null) {
    this.scopeLabels = scopeLabels;
    const anchors = this.anchors(nodes, currentScope);
    const previous = this.byId;
    const next: SimNode[] = [];
    const byId = new Map<string, SimNode>();
    const fresh: SimNode[] = [];
    for (const view of nodes) {
      const anchor = anchors.get(view.scope) ?? { x: 0, y: 0 };
      const old = previous.get(view.id);
      const node: SimNode = old ?? { id: view.id, r: 0, view, ax: 0, ay: 0, x: Number.NaN, y: Number.NaN, pull: 0 };
      node.view = view;
      node.r = radiusOf(view.degree, this.settings.nodeSize);
      node.ax = anchor.x;
      node.ay = anchor.y;
      if (!old) {
        const saved = this.local ? undefined : remembered.get(view.id);
        if (saved) {
          node.x = saved.x;
          node.y = saved.y;
        } else {
          fresh.push(node);
        }
      }
      next.push(node);
      byId.set(view.id, node);
    }

    const links: SimLink[] = [];
    const neighbours = new Map<string, Set<string>>();
    const touch = (a: string, b: string) => {
      let set = neighbours.get(a);
      if (!set) neighbours.set(a, (set = new Set()));
      set.add(b);
    };
    for (const view of edges) {
      const [source, target] = [byId.get(view.source), byId.get(view.target)];
      if (!source || !target) continue;
      links.push({ source, target, view });
      touch(view.source, view.target);
      touch(view.target, view.source);
    }

    // 新出现的点：从已经在图上的邻居旁边冒出来；没有邻居就落在自己那一簇的聚集点附近。
    // 偏移量由 id 定，同一份数据每次摆出来都一样。
    for (const node of fresh) {
      const h = hash(node.id);
      const angle = ((h % 3600) / 3600) * Math.PI * 2;
      const near = [...(neighbours.get(node.id) ?? [])].map((id) => byId.get(id)).find((n) => n && Number.isFinite(n.x));
      const [cx, cy, dist] = near ? [near.x, near.y, 30] : [node.ax, node.ay, 30 + ((h >>> 12) % 90)];
      node.x = cx + Math.cos(angle) * dist;
      node.y = cy + Math.sin(angle) * dist;
    }

    const first = this.nodes.length === 0;
    // 换了项目或范围（多出 / 少了整簇笔记）：画面要重新对一次。零星多一篇（agent 刚写的）不动视野，
    // 不然人正看着某一块，图自己跑了。
    const before = new Set(this.nodes.map((n) => n.view.scope));
    const after = new Set(next.map((n) => n.view.scope));
    const reshaped = !first && (before.size !== after.size || [...after].some((s) => !before.has(s)) || Math.abs(next.length - this.nodes.length) > Math.max(3, this.nodes.length * 0.3));
    const changed =
      first ||
      fresh.length > 0 ||
      next.length !== this.nodes.length ||
      links.length !== this.links.length ||
      links.some((l, i) => this.links[i]?.view.id !== l.view.id);
    this.nodes = next;
    this.links = links;
    this.byId = byId;
    this.neighbours = neighbours;
    if (this.hover && !byId.has(this.hover.id)) this.hover = null;
    this.hoverEdge = null;

    this.sim.nodes(next);
    this.applyForces();
    if (changed) {
      if (first || reducedMotion()) {
        // 第一次：先算到基本稳定再画，不让人看一场爆炸。
        this.sim.alpha(1);
        const ticks = Math.min(320, Math.max(60, Math.round(9000 / Math.max(12, next.length))));
        this.sim.tick(reducedMotion() ? 400 : ticks);
        this.sim.alpha(reducedMotion() ? 0 : 0.08);
        this.fitted = false;
      } else {
        this.sim.alpha(Math.max(this.sim.alpha(), 0.5));
      }
      if (!reducedMotion()) this.sim.restart();
    }
    if (!this.fitted && this.width > 0) this.fit(false);
    else if (reshaped) this.fitWhenSettled();
    this.invalidate();
  }

  /** 先按现在的位置对一次画面，布局快稳了再对一次（换了范围、局部图谱换了中心）。 */
  fitWhenSettled() {
    this.fit(true);
    this.refit = !reducedMotion();
  }

  /** 各 scope 的聚集点。两个以内（全局 + 当前项目）不分开；更多时当前项目居中，其余在一个圆上围一圈。 */
  private anchors(nodes: ViewNode[], currentScope: string | null): Map<string, { x: number; y: number }> {
    const counts = new Map<string, number>();
    for (const n of nodes) counts.set(n.scope, (counts.get(n.scope) ?? 0) + 1);
    const scopes = [...counts.keys()].sort();
    const out = new Map<string, { x: number; y: number }>();
    if (scopes.length <= 2 || this.local) {
      for (const s of scopes) out.set(s, { x: 0, y: 0 });
      return out;
    }
    const ring = scopes.filter((s) => s !== currentScope);
    if (currentScope && counts.has(currentScope)) out.set(currentScope, { x: 0, y: 0 });
    const radius = 140 + 18 * Math.sqrt(nodes.length) + 14 * ring.length;
    ring.forEach((scope, i) => {
      const angle = -Math.PI / 2 + (i / ring.length) * Math.PI * 2;
      out.set(scope, { x: Math.cos(angle) * radius, y: Math.sin(angle) * radius });
    });
    return out;
  }

  /** 按设置调各个力。相似建议只在「全图显示」时参与布局（局部图谱里只有中心那篇的，照常参与）。 */
  private applyForces() {
    const s = this.settings;
    const layoutLinks = this.links.filter((l) => l.view.kind !== "similar" || s.allSuggestions || this.local);
    for (const n of this.nodes) n.pull = 0;
    for (const l of layoutLinks) {
      l.source.pull += 1;
      l.target.pull += 1;
    }
    const strength = 0.1 + 0.9 * (s.linkStrength / 100);
    (this.sim.force("link") as ForceLink<SimNode, SimLink>)
      .links(layoutLinks)
      .distance((l) => (l.view.kind === "similar" ? 1.5 : 1) * s.linkDistance * (this.local ? 1.3 : 1))
      // 相似建议拉得轻：它只是把像的两篇凑近一点，不能把链接形成的结构扯散。
      .strength((l) => (l.view.kind === "similar" ? 0.04 + 0.16 * l.view.score : strength / Math.sqrt(Math.max(1, Math.min(l.source.pull, l.target.pull)))));
    (this.sim.force("charge") as ForceManyBody<SimNode>).strength(-(20 + 5.6 * s.repel));
    (this.sim.force("collide") as ForceCollide<SimNode>).radius((d) => d.r + 6);
    const base = 0.01 + 0.13 * (s.center / 100);
    // 没有链接的笔记拉得更松一点：被那一大团的斥力推到外圈，像 Obsidian 那样散成一圈。
    const pull = (d: SimNode) => base * (d.view.degree === 0 && !this.local ? 0.55 : 1);
    (this.sim.force("x") as ForceX<SimNode>).strength(pull);
    (this.sim.force("y") as ForceY<SimNode>).strength(pull);
  }

  private remember() {
    if (this.local) return;
    for (const n of this.nodes) if (Number.isFinite(n.x)) remembered.set(n.id, { x: n.x, y: n.y });
  }

  setSettings(next: GraphSettings) {
    const prev = this.settings;
    this.settings = next;
    if (prev.nodeSize !== next.nodeSize) for (const n of this.nodes) n.r = radiusOf(n.view.degree, next.nodeSize);
    const layout =
      prev.center !== next.center ||
      prev.repel !== next.repel ||
      prev.linkStrength !== next.linkStrength ||
      prev.linkDistance !== next.linkDistance ||
      prev.nodeSize !== next.nodeSize ||
      prev.allSuggestions !== next.allSuggestions;
    if (layout) {
      this.applyForces();
      if (this.nodes.length > 0) {
        if (reducedMotion()) {
          this.sim.alpha(1);
          this.sim.tick(300);
          this.sim.alpha(0);
        } else this.sim.alpha(Math.max(this.sim.alpha(), 0.35)).restart();
      }
    }
    this.invalidate();
  }

  /** 按条件着色：id → 颜色。没命中的点画成灰色；null 是没在着色，按范围上色。 */
  setColors(colors: Map<string, string> | null) {
    this.colors = colors;
    this.invalidate();
  }

  setSelected(id: string | null) {
    if (this.selected === id) return;
    this.selected = id;
    this.invalidate();
  }

  setHighlight(ids: Set<string> | null) {
    this.highlight = ids;
    this.invalidate();
  }

  /** 右边被抽屉盖住的宽度：适应窗口 / 居中时让开它。抽屉开合时整张图跟着挪半个抽屉宽，仍然居中在看得见的那一块里。 */
  setInsetRight(px: number) {
    const delta = px - this.insetRight;
    this.insetRight = px;
    if (delta === 0 || !this.fitted) return;
    // 没动过画面：在看得见的那一块里重新适应（图比剩下的宽度宽时，只挪会把一边推出去）。
    if (this.autoFit) {
      this.fit(true);
      return;
    }
    const base = this.anim?.to ?? this.view;
    this.moveTo({ ...base, x: base.x - delta / 2 }, true);
  }

  resize(width: number, height: number) {
    const dpr = window.devicePixelRatio || 1;
    if (width === this.width && height === this.height && dpr === this.dpr) return;
    // 窗口变大小：保持画面中心不动。
    if (this.width > 0 && this.fitted) {
      this.view.x += (width - this.width) / 2;
      this.view.y += (height - this.height) / 2;
    }
    this.width = width;
    this.height = height;
    this.dpr = dpr;
    this.canvas.width = Math.round(width * dpr);
    this.canvas.height = Math.round(height * dpr);
    if (!this.fitted && this.nodes.length > 0) this.fit(false);
    this.invalidate();
  }

  // ---- 视图 ----

  private availW() {
    return Math.max(120, this.width - this.insetRight);
  }

  /** 把整张图放进可见区域。 */
  fit(animate = true) {
    if (this.width <= 0 || this.nodes.length === 0) return;
    let [minX, minY, maxX, maxY] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const n of this.nodes) {
      minX = Math.min(minX, n.x - n.r);
      maxX = Math.max(maxX, n.x + n.r);
      minY = Math.min(minY, n.y - n.r);
      maxY = Math.max(maxY, n.y + n.r);
    }
    const availW = this.availW();
    // 四周留白：边上的点下面还要放得下标签。
    const pad = this.local ? 26 : 44;
    const maxK = this.local ? 1.6 : 2;
    const k = clamp(Math.min((availW - pad * 2) / Math.max(1, maxX - minX), (this.height - pad * 2) / Math.max(1, maxY - minY)), MIN_K, maxK);
    const to = { k, x: availW / 2 - ((minX + maxX) / 2) * k, y: this.height / 2 - ((minY + maxY) / 2) * k };
    this.fitted = true;
    this.moveTo(to, animate);
    this.autoFit = true;
  }

  zoomBy(factor: number) {
    this.autoFit = false;
    this.zoomAt(this.availW() / 2, this.height / 2, this.view.k * factor, true);
  }

  /** 把一篇笔记挪到可见区域中间（选中列表里的一项、点了抽屉里的链接）。 */
  focus(id: string) {
    const node = this.byId.get(id);
    if (!node) return;
    this.autoFit = false;
    const k = Math.max(this.view.k, 0.9);
    this.moveTo({ k, x: this.availW() / 2 - node.x * k, y: this.height / 2 - node.y * k }, true);
  }

  private zoomAt(sx: number, sy: number, k: number, animate: boolean) {
    const next = clamp(k, MIN_K, MAX_K);
    const wx = (sx - this.view.x) / this.view.k;
    const wy = (sy - this.view.y) / this.view.k;
    this.moveTo({ k: next, x: sx - wx * next, y: sy - wy * next }, animate);
  }

  private moveTo(to: ViewTransform, animate: boolean) {
    if (!animate || reducedMotion()) {
      this.anim = null;
      this.view = to;
    } else {
      this.anim = { from: { ...this.view }, to, start: performance.now(), ms: 220 };
    }
    this.invalidate();
  }

  // ---- 颜色 / 大小 ----

  private colorOf(n: SimNode): string {
    if (n.view.ghost) return GROUP_COLOR.ghost;
    if (this.colors) return this.colors.get(n.id) ?? MUTED;
    return GROUP_COLOR[n.view.group];
  }

  /** 点在屏幕上画多大（像素）。 */
  private screenR(n: SimNode): number {
    return Math.max(1.5, n.r * Math.sqrt(this.view.k));
  }

  /** 这条线画不画：相似建议只画悬停 / 选中那篇的，除非设置里打开了全图显示。 */
  private edgeShown(l: SimLink, focusId: string | null): boolean {
    if (l.view.kind !== "similar" || this.settings.allSuggestions || this.local) return true;
    return !!focusId && (l.source.id === focusId || l.target.id === focusId);
  }

  // ---- 命中 ----

  private toWorld(sx: number, sy: number) {
    return { x: (sx - this.view.x) / this.view.k, y: (sy - this.view.y) / this.view.k };
  }

  /** 离指针最近的点。命中范围比画出来的大（屏幕上至少 24px 宽），小点也好点中；指着标签也算。 */
  private nodeAt(sx: number, sy: number): SimNode | null {
    return this.circleAt(sx, sy) ?? this.labelHits.find((b) => sx >= b.x1 && sx <= b.x2 && sy >= b.y1 && sy <= b.y2 && this.byId.get(b.node.id) === b.node)?.node ?? null;
  }

  private circleAt(sx: number, sy: number): SimNode | null {
    const { k, x: ox, y: oy } = this.view;
    let best: SimNode | null = null;
    let bestDist = Infinity;
    for (const n of this.nodes) {
      const reach = Math.max(this.screenR(n) + 4, 12);
      const d = Math.hypot(n.x * k + ox - sx, n.y * k + oy - sy);
      if (d <= reach && d < bestDist) {
        best = n;
        bestDist = d;
      }
    }
    return best;
  }

  private edgeAt(sx: number, sy: number): SimLink | null {
    if (this.local) return null;
    const { k, x: ox, y: oy } = this.view;
    const focusId = this.hover?.id ?? this.selected;
    let best: SimLink | null = null;
    let bestDist = 6;
    for (const l of this.links) {
      if (!this.edgeShown(l, focusId)) continue;
      const [ax, ay, bx, by] = [l.source.x * k + ox, l.source.y * k + oy, l.target.x * k + ox, l.target.y * k + oy];
      const len2 = (bx - ax) ** 2 + (by - ay) ** 2;
      const t = len2 === 0 ? 0 : clamp(((sx - ax) * (bx - ax) + (sy - ay) * (by - ay)) / len2, 0, 1);
      const d = Math.hypot(sx - (ax + t * (bx - ax)), sy - (ay + t * (by - ay)));
      if (d < bestDist) {
        best = l;
        bestDist = d;
      }
    }
    return best;
  }

  /**
   * 指针在画布里的位置，换算成布局像素（画布内部、点的坐标都用它）。
   * 祖先上可能有 CSS zoom（设置里的「字号」就是给整窗加 zoom）：这时 clientX 和 getBoundingClientRect 是缩放后的视口像素，
   * 直接相减会越往右下偏得越多——指着点却悬停不上。按画布的视口宽 / 布局宽折回去。
   */
  private pointIn(e: { clientX: number; clientY: number }) {
    const rect = this.canvas.getBoundingClientRect();
    const sx = this.width > 0 && rect.width > 0 ? rect.width / this.width : 1;
    const sy = this.height > 0 && rect.height > 0 ? rect.height / this.height : 1;
    return { x: (e.clientX - rect.left) / sx, y: (e.clientY - rect.top) / sy };
  }

  // ---- 指针 ----

  private onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    const { x, y } = this.pointIn(e);
    this.pointer = { x, y };
    const node = this.nodeAt(x, y);
    this.canvas.setPointerCapture(e.pointerId);
    this.anim = null;
    this.cb.onTip(null);
    if (node && e.altKey && !this.local && !node.view.ghost && node.view.note?.writable) {
      this.drag = { type: "link", from: node, x, y, startX: x, startY: y };
      this.cb.onCursor("crosshair");
    } else if (node) {
      this.drag = { type: "node", node, moved: false, startX: x, startY: y };
    } else {
      this.drag = { type: "pan", moved: false, startX: x, startY: y, view: { ...this.view } };
    }
  };

  private onPointerMove = (e: PointerEvent) => {
    const { x, y } = this.pointIn(e);
    this.pointer = { x, y };
    const drag = this.drag;
    if (!drag) {
      if (this.updateHover(true)) this.invalidate();
      return;
    }
    const far = Math.hypot(x - drag.startX, y - drag.startY) > 3;
    if (drag.type === "node") {
      if (!drag.moved && !far) return;
      if (!drag.moved) {
        drag.moved = true;
        this.cb.onCursor("grabbing");
        // 拖起来的时候别的点跟着让位；松手再慢慢停下。
        if (!reducedMotion()) this.sim.alphaTarget(0.25).restart();
      }
      const w = this.toWorld(x, y);
      drag.node.fx = w.x;
      drag.node.fy = w.y;
      if (reducedMotion()) {
        drag.node.x = w.x;
        drag.node.y = w.y;
      }
    } else if (drag.type === "pan") {
      if (!drag.moved && !far) return;
      drag.moved = true;
      this.autoFit = false;
      this.cb.onCursor("grabbing");
      this.view = { k: drag.view.k, x: drag.view.x + (x - drag.startX), y: drag.view.y + (y - drag.startY) };
    } else {
      drag.x = x;
      drag.y = y;
      const over = this.nodeAt(x, y);
      this.hover = over && over !== drag.from && !over.view.ghost ? over : null;
    }
    this.invalidate();
  };

  private onPointerUp = (e: PointerEvent) => {
    const drag = this.drag;
    this.drag = null;
    if (this.canvas.hasPointerCapture(e.pointerId)) this.canvas.releasePointerCapture(e.pointerId);
    if (!drag) return;
    const { x, y } = this.pointIn(e);
    this.pointer = e.pointerType === "mouse" ? { x, y } : null;
    if (drag.type === "node") {
      drag.node.fx = null;
      drag.node.fy = null;
      this.sim.alphaTarget(0);
      if (!drag.moved) {
        this.quietTip = drag.node.id;
        this.cb.onSelect(drag.node.view);
      } else this.remember();
    } else if (drag.type === "pan") {
      if (!drag.moved && !this.local) {
        const edge = this.edgeAt(x, y);
        if (edge) {
          this.quietTip = `edge:${edge.view.id}`;
          this.cb.onEdge(edge.view, edge.source.view, edge.target.view, { x, y });
        } else this.cb.onSelect(null);
      }
    } else {
      const over = this.nodeAt(x, y);
      if (over && over !== drag.from && !over.view.ghost) this.cb.onLink(drag.from.view, over.view);
    }
    this.updateHover(true);
    this.invalidate();
  };

  private onPointerLeave = () => {
    if (this.drag) return;
    this.pointer = null;
    this.hover = null;
    this.hoverEdge = null;
    this.cb.onTip(null);
    this.cb.onCursor("default");
    this.invalidate();
  };

  /**
   * 重新判断指针底下是哪篇 / 哪条线，返回有没有换。
   * `moved`：指针自己动了，提示要跟着更新；否则是图在指针底下动了，只有换了目标才更新提示。
   */
  private updateHover(moved: boolean): boolean {
    const p = this.pointer;
    if (!p) return false;
    const { x, y } = p;
    const node = this.nodeAt(x, y);
    const edge = node ? null : this.edgeAt(x, y);
    const changed = node !== this.hover || edge !== this.hoverEdge;
    if (!changed && !moved) return false;
    this.hover = node;
    this.hoverEdge = edge;
    this.cb.onCursor(node || edge ? "pointer" : "default");
    const under = node ? node.id : edge ? `edge:${edge.view.id}` : null;
    if (under !== this.quietTip) this.quietTip = null;
    if (this.quietTip) this.cb.onTip(null);
    else if (node) {
      // 提示放在点的右边、和点齐平：不压住点下面的标签，也不挡住指针要去的地方。
      // 标签在点下面居中，提示框要让过标签的半宽。
      const { k, x: ox, y: oy } = this.view;
      const [sx, sy, sr] = [node.x * k + ox, node.y * k + oy, this.screenR(node)];
      this.ctx.font = `600 11px ${FONT}`;
      const off = Math.max(sr, this.ctx.measureText(this.fitLabel(node.view.label, 230, true)).width / 2) + 10;
      this.cb.onTip({ x: sx + off, y: sy - 14, left: sx - off, node: node.view });
    } else if (edge) this.cb.onTip({ x: x + 14, y: y + 14, left: x - 14, edge: { view: edge.view, source: edge.source.view, target: edge.target.view } });
    else this.cb.onTip(null);
    return changed;
  }

  private onDoubleClick = (e: MouseEvent) => {
    const { x, y } = this.pointIn(e);
    const node = this.nodeAt(x, y);
    if (node) {
      this.autoFit = false;
      this.zoomAt(x, y, Math.max(this.view.k * 1.6, 1.4), true);
    } else this.fit();
  };

  /** 滚动是平移，按住 ⌘ / ctrl（或触控板捏合）是缩放——和画布类工具一致。局部图谱里不按修饰键的滚动留给抽屉。 */
  private onWheel = (e: WheelEvent) => {
    const zoom = e.ctrlKey || e.metaKey;
    if (this.local && !zoom) return;
    e.preventDefault();
    const { x, y } = this.pointIn(e);
    this.pointer = { x, y };
    this.anim = null;
    this.autoFit = false;
    if (zoom) {
      this.zoomAt(x, y, this.view.k * Math.exp(-e.deltaY * 0.01), false);
    } else {
      this.view = { ...this.view, x: this.view.x - e.deltaX, y: this.view.y - e.deltaY };
      this.invalidate();
    }
    this.cb.onTip(null);
  };

  private onGestureStart = (e: Event) => {
    e.preventDefault();
    this.autoFit = false;
    this.gestureK = this.view.k;
  };

  private onGestureChange = (e: Event & { scale?: number; clientX?: number; clientY?: number }) => {
    e.preventDefault();
    const at = e.clientX === undefined || e.clientY === undefined ? { x: this.width / 2, y: this.height / 2 } : this.pointIn({ clientX: e.clientX, clientY: e.clientY });
    this.zoomAt(at.x, at.y, this.gestureK * (e.scale ?? 1), false);
  };

  // ---- 绘制 ----

  private invalidate() {
    if (this.frame || this.disposed) return;
    this.frame = requestAnimationFrame((now) => {
      this.frame = 0;
      if (this.anim) {
        const t = Math.min(1, (now - this.anim.start) / this.anim.ms);
        const ease = 1 - (1 - t) ** 3;
        const { from, to } = this.anim;
        this.view = { k: from.k + (to.k - from.k) * ease, x: from.x + (to.x - from.x) * ease, y: from.y + (to.y - from.y) * ease };
        if (t >= 1) this.anim = null;
        else this.invalidate();
      }
      // 图在静止的指针底下动了（布局还在收、平移、缩放动画）：先按这一帧的位置重新判断悬停再画。
      if (!this.drag) this.updateHover(false);
      this.draw();
    });
  }

  /** 这一帧谁是「主角」：悬停的那篇和它的邻居；没有悬停就是选中的那篇。 */
  private focusSet(): { ids: Set<string>; strong: boolean } | null {
    const id = this.hover?.id ?? this.selected;
    if (!id || !this.byId.has(id)) return null;
    const ids = new Set<string>([id, ...(this.neighbours.get(id) ?? [])]);
    return { ids, strong: !!this.hover };
  }

  private draw() {
    const { ctx, view, settings: s } = this;
    const { k } = view;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.width, this.height);
    if (this.nodes.length === 0) return;

    const focus = this.focusSet();
    const focusId = this.hover?.id ?? this.selected;
    const focusNode = focusId ? this.byId.get(focusId) : undefined;
    const linking = this.drag?.type === "link" ? this.drag.from : null;
    const nodeAlpha = (n: SimNode) => {
      // 拖线时起点和落点都得看得清。
      if (n === this.hover || n.id === this.selected || n === linking) return 1;
      if (focus?.strong) return focus.ids.has(n.id) ? 1 : 0.16;
      if (this.highlight) return this.highlight.has(n.id) ? 1 : 0.16;
      if (focus) return focus.ids.has(n.id) ? 1 : 0.5;
      return 1;
    };

    ctx.save();
    ctx.translate(view.x, view.y);
    ctx.scale(k, k);
    const px = 1 / k;
    const lw = s.lineWidth / 100;
    // 主角的链接用主角的颜色（灰点、悬空的点用深灰）。
    const focusColor = focusNode ? this.colorOf(focusNode) : LINE_STRONG;
    const hotColor = focusColor === MUTED || focusColor === GROUP_COLOR.ghost ? LINE_STRONG : focusColor;

    // 连线：先画不相干的，再画和主角相连的（压在上面）。
    for (const pass of [0, 1] as const) {
      for (const l of this.links) {
        if (!this.edgeShown(l, focusId)) continue;
        const touches = !!focusId && (l.source.id === focusId || l.target.id === focusId);
        const hot = touches || l === this.hoverEdge;
        if ((pass === 1) !== hot) continue;
        const kind = l.view.kind;
        ctx.globalAlpha = hot ? 1 : focus?.strong || this.highlight ? 0.25 : focus ? 0.6 : 1;
        ctx.strokeStyle = !hot ? (kind === "similar" ? SUGGEST : LINE) : kind === "link" && touches ? hotColor : LINE_STRONG;
        ctx.lineWidth = (hot ? 1.6 : 1) * lw * px;
        ctx.setLineDash(kind !== "link" ? [4 * px, 4 * px] : []);
        ctx.beginPath();
        ctx.moveTo(l.source.x, l.source.y);
        ctx.lineTo(l.target.x, l.target.y);
        ctx.stroke();
        // 箭头：链接有方向（谁引用谁）。默认只给主角的线画；设置里打开后缩得不太小时都画。
        if (kind === "link" && (hot || (s.arrows && k >= 0.6))) {
          ctx.setLineDash([]);
          ctx.fillStyle = ctx.strokeStyle;
          this.arrow(l.source, l.target, px);
          if (l.view.mutual) this.arrow(l.target, l.source, px);
        }
      }
    }
    ctx.setLineDash([]);

    // ⌥ 拖线。
    if (this.drag?.type === "link") {
      const to = this.hover ?? this.toWorld(this.drag.x, this.drag.y);
      ctx.globalAlpha = 1;
      ctx.strokeStyle = INK;
      ctx.lineWidth = 1.5 * px;
      ctx.setLineDash([5 * px, 4 * px]);
      ctx.beginPath();
      ctx.moveTo(this.drag.from.x, this.drag.from.y);
      ctx.lineTo(to.x, to.y);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    for (const n of this.nodes) {
      const alpha = nodeAlpha(n);
      const color = this.colorOf(n);
      const sr = this.screenR(n);
      const r = sr * px;
      const lifted = n === this.hover || n.id === this.selected;
      // 底下垫一圈底色：点叠在线上、点和点挨着时分得开。
      ctx.globalAlpha = 1;
      ctx.fillStyle = SURFACE;
      ctx.beginPath();
      ctx.arc(n.x, n.y, r + 1.5 * px, 0, Math.PI * 2);
      ctx.fill();
      ctx.globalAlpha = alpha;
      ctx.beginPath();
      if (n.view.ghost) {
        ctx.arc(n.x, n.y, Math.max(r - 0.75 * px, px), 0, Math.PI * 2);
        ctx.strokeStyle = color;
        ctx.lineWidth = 1.5 * px;
        ctx.setLineDash([3 * px, 3 * px]);
        ctx.stroke();
        ctx.setLineDash([]);
      } else if (n.view.hollow) {
        // 空心环的粗细跟着点大小走，小点也看得出是空心。
        const ring = clamp(sr * 0.32, 1.25, 2.5);
        ctx.arc(n.x, n.y, Math.max(r - (ring / 2) * px, px), 0, Math.PI * 2);
        ctx.strokeStyle = color;
        ctx.lineWidth = ring * px;
        ctx.stroke();
      } else {
        ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
        ctx.fillStyle = color;
        ctx.fill();
      }
      if (lifted) {
        // 悬停 / 选中：外面再套一圈深色，不改它本来的颜色。
        ctx.globalAlpha = n.id === this.selected ? 1 : 0.55;
        ctx.strokeStyle = INK;
        ctx.lineWidth = 2 * px;
        ctx.beginPath();
        ctx.arc(n.x, n.y, r + 4 * px, 0, Math.PI * 2);
        ctx.stroke();
      }
    }
    ctx.restore();
    ctx.globalAlpha = 1;
    this.drawLabels(nodeAlpha, focus?.ids ?? null);
  }

  private arrow(from: SimNode, to: SimNode, px: number) {
    const { ctx } = this;
    const [dx, dy] = [to.x - from.x, to.y - from.y];
    const len = Math.hypot(dx, dy);
    const toR = this.screenR(to) * px;
    if (len < toR + this.screenR(from) * px + 10 * px) return;
    const [ux, uy] = [dx / len, dy / len];
    const tipX = to.x - ux * (toR + 2 * px);
    const tipY = to.y - uy * (toR + 2 * px);
    const size = 6 * px;
    ctx.beginPath();
    ctx.moveTo(tipX, tipY);
    ctx.lineTo(tipX - ux * size - uy * size * 0.5, tipY - uy * size + ux * size * 0.5);
    ctx.lineTo(tipX - ux * size + uy * size * 0.5, tipY - uy * size - ux * size * 0.5);
    ctx.closePath();
    ctx.fill();
  }

  /**
   * 文字在屏幕坐标里画（字号不随缩放变）。先给重要的占位，放不下的不画——宁可少标也不叠字。
   * 随缩放出现：点按线性缩放该有多大（r × k）过了门槛才标，门槛由设置里的「标签」定；在门槛附近淡入。
   * 悬停 / 选中的那篇、它的邻居、搜索命中的不受门槛限制。
   */
  private drawLabels(alphaOf: (n: SimNode) => number, focus: Set<string> | null) {
    const { ctx, view } = this;
    const { k } = view;
    const placed: { x1: number; y1: number; x2: number; y2: number }[] = [];
    this.labelHits = [];
    const free = (x1: number, y1: number, x2: number, y2: number) => !placed.some((r) => x1 < r.x2 && x2 > r.x1 && y1 < r.y2 && y2 > r.y1);
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.lineJoin = "round";

    // 项目名：三个以上 scope 时标在各自那一簇的上面。
    if (this.scopeLabels.size > 0) {
      const boxes = new Map<string, { minX: number; maxX: number; minY: number }>();
      // 按有链接的那几篇定位：没链接的散在外圈，算进来框就撑到天边，名字落不到那一簇上。
      const core = new Set(this.nodes.filter((n) => !n.view.ghost && n.view.degree > 0).map((n) => n.view.scope));
      for (const n of this.nodes) {
        if (n.view.ghost || (core.has(n.view.scope) && n.view.degree === 0)) continue;
        const b = boxes.get(n.view.scope);
        if (!b) boxes.set(n.view.scope, { minX: n.x, maxX: n.x, minY: n.y - n.r });
        else {
          b.minX = Math.min(b.minX, n.x);
          b.maxX = Math.max(b.maxX, n.x);
          b.minY = Math.min(b.minY, n.y - n.r);
        }
      }
      ctx.font = `600 11px ${FONT}`;
      for (const [scope, b] of boxes) {
        const text = this.scopeLabels.get(scope);
        if (!text) continue;
        const sx = ((b.minX + b.maxX) / 2) * k + view.x;
        const sy = b.minY * k + view.y - 24;
        const w = ctx.measureText(text).width;
        placed.push({ x1: sx - w / 2 - 4, y1: sy - 2, x2: sx + w / 2 + 4, y2: sy + 15 });
        ctx.globalAlpha = 0.9;
        ctx.strokeStyle = SURFACE;
        ctx.lineWidth = 3;
        ctx.strokeText(text, sx, sy);
        ctx.fillStyle = INK_4;
        ctx.fillText(text, sx, sy);
      }
    }

    const level = this.settings.labels;
    // 门槛：点按线性缩放在屏幕上有多大（像素）才标。局部图谱点少，全标。
    const threshold = this.local ? 0 : level <= 0 ? Infinity : 1 + 8 * (1 - level / 100);
    const order = [...this.nodes].sort((a, b) => this.labelRank(b, focus) - this.labelRank(a, focus));
    for (const n of order) {
      const forced = n === this.hover || n.id === this.selected;
      let alpha = alphaOf(n);
      if (!forced) {
        if (alpha < 0.3) continue;
        if (!focus?.has(n.id) && !this.highlight?.has(n.id)) {
          const fade = clamp((n.r * k - threshold) / 2 + 1, 0, 1);
          if (fade < 0.1) continue;
          alpha *= fade;
        }
      }
      ctx.font = `${forced ? 600 : 500} 11px ${FONT}`;
      // 图上只放得下短标题：悬停 / 选中的那篇放宽一些，全文在悬停提示和抽屉里。
      const text = this.fitLabel(n.view.label, forced ? 230 : this.local ? 120 : 148, forced);
      const w = ctx.measureText(text).width;
      const sx = n.x * k + view.x;
      const sy = n.y * k + view.y + this.screenR(n) + 4;
      if (sx + w / 2 < 0 || sx - w / 2 > this.width || sy < -14 || sy > this.height) continue;
      const box = { x1: sx - w / 2 - 3, y1: sy - 1, x2: sx + w / 2 + 3, y2: sy + 14 };
      if (!forced && !free(box.x1, box.y1, box.x2, box.y2)) continue;
      placed.push(box);
      this.labelHits.push({ node: n, ...box });
      ctx.globalAlpha = alpha;
      ctx.strokeStyle = SURFACE;
      ctx.lineWidth = 3;
      ctx.strokeText(text, sx, sy);
      // 文字用墨色，不跟点的颜色走：浅色的点配同色的字读不清。
      ctx.fillStyle = forced ? INK : n.view.ghost ? INK_4 : INK_2;
      ctx.fillText(text, sx, sy);
    }
    ctx.globalAlpha = 1;
  }

  /** 按像素宽度截断（中文一个字比英文字母宽得多，按字数截会一长一短）。结果记下来，不用每帧重量。 */
  private fitLabel(label: string, maxWidth: number, bold: boolean): string {
    const key = `${bold ? "b" : "n"}${maxWidth}:${label}`;
    const cached = this.labelCache.get(key);
    if (cached !== undefined) return cached;
    const { ctx } = this;
    let text = label;
    if (ctx.measureText(text).width > maxWidth) {
      const chars = [...label];
      let lo = 1;
      let hi = chars.length;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (ctx.measureText(`${chars.slice(0, mid).join("")}…`).width <= maxWidth) lo = mid;
        else hi = mid - 1;
      }
      text = `${chars.slice(0, lo).join("").trimEnd()}…`;
    }
    if (this.labelCache.size > 4000) this.labelCache.clear();
    this.labelCache.set(key, text);
    return text;
  }

  private labelRank(n: SimNode, focus: Set<string> | null): number {
    if (n === this.hover) return 1e6;
    if (n.id === this.selected) return 1e5;
    const bonus = (focus?.has(n.id) ? 1e4 : 0) + (this.highlight?.has(n.id) ? 1e3 : 0);
    return bonus + n.view.degree * 10 + (n.view.hollow ? 0 : 5);
  }
}

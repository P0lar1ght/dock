import assert from "node:assert/strict";
import { test } from "node:test";

import type { El } from "../src/geometry.ts";
import { graphPositions, isLive, LIVE_PREFIX, livePlan } from "../src/live.ts";

const el = (o: Partial<El> & { id: string; type: string }): El => ({ x: 0, y: 0, width: 100, height: 50, ...o }) as El;

test("edit add：给了坐标照画，颜色 / 标签换成 Excalidraw 字段，id 带草稿前缀", () => {
  const plan = livePlan({ add: [{ type: "rectangle", id: "a", x: 10, y: 20, text: "登录", stroke: "#ff0000", fill: "#00ff00", strokeStyle: "dashed" }] }, true, []);
  assert.deepEqual(plan.skeletons, [
    {
      type: "rectangle",
      id: `${LIVE_PREFIX}a`,
      x: 10,
      y: 20,
      width: 180,
      height: 80,
      strokeColor: "#ff0000",
      backgroundColor: "#00ff00",
      fillStyle: "solid",
      strokeStyle: "dashed",
      label: { text: "登录" },
    },
  ]);
  assert.ok(isLive(plan.skeletons[0] as { id: string }));
  assert.deepEqual(plan.pointer, { x: 100, y: 60 });
  assert.equal(plan.status, "Agent 正在画 1 个图形…");
});

test("edit add：还在写时最后一项先不画", () => {
  const args = { add: [{ type: "ellipse", x: 0, y: 0 }, { type: "rect" }] };
  assert.equal(livePlan(args, false, []).skeletons.length, 1);
  // 写完了：认不出的类型照样不画（服务器会报错）。
  assert.equal(livePlan(args, true, []).skeletons.length, 1);
});

test("edit add：没给坐标照服务器 free_slot 排（空板从 100,100 起，下一个排在右边）", () => {
  const plan = livePlan({ add: [{ type: "rectangle" }, { type: "rectangle" }] }, true, []);
  assert.deepEqual(
    plan.skeletons.map((s) => [s.x, s.y]),
    [
      [100, 100],
      [400, 100],
    ],
  );
  // 已有内容：排在它右边 2×GAP。
  const beside = livePlan({ add: [{ type: "text", text: "hi" }] }, true, [el({ id: "b", type: "rectangle", x: 0, y: 40, width: 200, height: 80 })]);
  assert.deepEqual([beside.skeletons[0].x, beside.skeletons[0].y], [320, 40]);
  assert.equal(beside.skeletons[0].text, "hi");
});

test("edit add：箭头两端都给了、没给坐标，从边连到边", () => {
  const scene = [el({ id: "a", type: "rectangle", x: 0, y: 0, width: 100, height: 100 }), el({ id: "b", type: "rectangle", x: 300, y: 0, width: 100, height: 100 })];
  const plan = livePlan({ add: [{ type: "arrow", from: "a", to: "b" }] }, true, scene);
  const arrow = plan.skeletons[0];
  assert.equal(arrow.x, 108);
  assert.equal(arrow.y, 50);
  assert.deepEqual(arrow.points, [
    [0, 0],
    [184, 0],
  ]);
});

test("edit update / delete：指着要动的现有图形，不画草稿", () => {
  const scene = [el({ id: "a", type: "rectangle", x: 0, y: 0, width: 100, height: 100 }), el({ id: "b", type: "rectangle", x: 300, y: 0 })];
  const plan = livePlan({ delete: ["a", "nope"], update: [{ id: "b", text: "x" }] }, true, scene);
  assert.deepEqual(plan.targets, ["a", "b"]);
  assert.deepEqual(plan.pointer, { x: 350, y: 25 });
  assert.equal(plan.skeletons.length, 0);
  assert.equal(plan.status, "Agent 正在删 2 个、改 1 个图形…");
});

test("草稿和删掉的不算现有内容", () => {
  const scene = [el({ id: `${LIVE_PREFIX}x`, type: "rectangle", x: 0, y: 0 }), el({ id: "gone", type: "rectangle", x: 0, y: 0, isDeleted: true })];
  const plan = livePlan({ add: [{ type: "rectangle" }] }, true, scene);
  assert.deepEqual([plan.skeletons[0].x, plan.skeletons[0].y], [100, 100]);
});

test("graph：照服务器分层（a→b 两层），边从节点的边连到边", () => {
  const plan = livePlan(
    {
      nodes: [
        { id: "a", text: "开始" },
        { id: "b", type: "diamond" },
      ],
      edges: [{ from: "a", to: "b", text: "是" }],
    },
    true,
    [],
  );
  const [a, b, edge] = plan.skeletons;
  assert.deepEqual([a.x, a.y, a.width, a.height, a.label], [100, 100, 200, 80, { text: "开始" }]);
  assert.deepEqual([b.type, b.x, b.y], ["diamond", 100, 260]);
  assert.equal(edge.type, "arrow");
  assert.deepEqual(edge.label, { text: "是" });
  assert.equal(edge.x, 200);
  assert.equal(edge.y, 188);
  assert.equal(plan.status, "Agent 正在画流程图：2 个节点、1 条边");
});

test("graphPositions：回边不改层；row / grid 按实际宽度排", () => {
  const nodes = [
    { id: "a", w: 200, h: 80 },
    { id: "b", w: 100, h: 40 },
    { id: "c", w: 200, h: 80 },
  ];
  const cfg = { columns: 2, gapX: 60, gapY: 80, originX: 0, originY: 0 };
  assert.deepEqual(
    graphPositions(nodes, [{ from: "a", to: "b" }, { from: "b", to: "c" }, { from: "c", to: "a" }], { ...cfg, kind: "layered" }),
    [
      [0, 0],
      [0, 160],
      [0, 280],
    ],
  );
  assert.deepEqual(graphPositions(nodes, [], { ...cfg, kind: "row" }), [
    [0, 0],
    [260, 20],
    [420, 0],
  ]);
  assert.deepEqual(graphPositions(nodes, [], { ...cfg, kind: "grid" }), [
    [0, 0],
    [260, 20],
    [0, 160],
  ]);
});

test("排版类：按 ids 指着现有图形", () => {
  const scene = [el({ id: "a", type: "rectangle", x: 0, y: 0, width: 100, height: 100 }), el({ id: "b", type: "rectangle", x: 200, y: 0, width: 100, height: 100 })];
  const plan = livePlan({ ids: ["a", "b", "zz"], align: "top" }, true, scene);
  assert.deepEqual(plan.targets, ["a", "b"]);
  assert.deepEqual(plan.pointer, { x: 250, y: 50 });
  assert.equal(plan.status, "Agent 正在调整 2 个图形…");
  // 还在写：最后一个 id 可能没写完。
  assert.deepEqual(livePlan({ ids: ["a", "b"] }, false, scene).targets, ["a"]);
});

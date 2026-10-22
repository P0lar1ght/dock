import assert from "node:assert/strict";
import { test } from "node:test";

import { brief, idsOf, intersects, line, shapes, snapshotElements, viewportOf, type El } from "../src/geometry.ts";

const el = (o: Partial<El> & { id: string; type: string }): El => ({ x: 0, y: 0, width: 100, height: 50, ...o }) as El;

const box = el({ id: "a", type: "rectangle", x: 10, y: 20, width: 180, height: 80 });
const label = el({ id: "a-label", type: "text", containerId: "a", text: "登录" });
const other = el({ id: "b", type: "rectangle", x: 400, y: 0 });
const arrow = el({ id: "e1", type: "arrow", x: 190, y: 60, width: 210, height: 0, startBinding: { elementId: "a" }, endBinding: { elementId: "b" } });
const arrowLabel = el({ id: "e1-label", type: "text", containerId: "e1", text: "成功" });
const loose = el({ id: "e2", type: "arrow", startBinding: { elementId: "a" }, endBinding: { elementId: "c" } });
const gone = el({ id: "d", type: "ellipse", isDeleted: true });
const all = [box, label, other, arrow, arrowLabel, loose, gone];

test("brief / line carry the container label and normalise negative sizes", () => {
  assert.deepEqual(brief(box, all), { id: "a", type: "rectangle", label: "登录", x: 10, y: 20, width: 180, height: 80 });
  const flipped = brief(el({ id: "f", type: "rectangle", x: 100, y: 100, width: -40, height: -20 }), all);
  assert.deepEqual([flipped.x, flipped.y, flipped.width, flipped.height], [60, 80, 40, 20]);
  assert.equal(line(brief(box, all)), "rectangle a「登录」 (10,20) 180×80");
});

test("shapes drops container texts and deleted elements", () => {
  assert.deepEqual(shapes(all).map((e) => e.id), ["a", "b", "e1", "e2"]);
});

test("idsOf keeps non-empty strings only", () => {
  assert.deepEqual(idsOf({ ids: ["a", "", 3, "b"] }), ["a", "b"]);
  assert.deepEqual(idsOf({ ids: "a" }), []);
  assert.deepEqual(idsOf(undefined), []);
});

test("viewportOf converts scroll and zoom to canvas coordinates; intersects uses bounding boxes", () => {
  const view = viewportOf({ scrollX: -100, scrollY: 50, width: 800, height: 600, zoom: { value: 2 } });
  assert.deepEqual(view, { x: 100, y: -50, width: 400, height: 300, zoom: 2 });
  assert.equal(intersects(box, view), true, "从 10 到 190，跨过 100");
  assert.equal(intersects(other, view), true, "400 正好在右边界上");
  assert.equal(intersects(el({ id: "far", type: "rectangle", x: 600, y: 0 }), view), false);
  assert.equal(intersects(el({ id: "neg", type: "rectangle", x: 99, y: 0, width: -50 }), view), false, "负宽度按包围盒算");
});

test("snapshotElements takes picked shapes, their labels, and arrows with both ends picked", () => {
  assert.deepEqual(snapshotElements(all, []).map((e) => e.id), ["a", "a-label", "b", "e1", "e1-label", "e2"]);
  assert.deepEqual(snapshotElements(all, ["a"]).map((e) => e.id), ["a", "a-label"]);
  assert.deepEqual(snapshotElements(all, ["a", "b"]).map((e) => e.id), ["a", "a-label", "b", "e1", "e1-label"]);
  assert.deepEqual(snapshotElements(all, ["a-label"]).map((e) => e.id), [], "标签文字本身不算选中的图形");
  assert.deepEqual(snapshotElements(all, ["d"]), [], "删掉的不截");
});

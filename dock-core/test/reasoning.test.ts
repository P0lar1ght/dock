import assert from "node:assert/strict";
import { test } from "node:test";
import { visibleReasoningText } from "../src/reasoning.ts";

test("strips full signature block", () => {
  assert.equal(
    visibleReasoningText("plan <signature>abc</signature> more").trim(),
    "plan  more".trim() || "plan  more",
  );
  assert.equal(visibleReasoningText("plan <signature>abc</signature> more"), "plan  more");
});

test("orphan close tag keeps only summary (screenshot leak)", () => {
  const raw =
    "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDxKPiNBnLBNry-_0wFAUfhbZ8akGc-</signature>\n用户问有哪些未提交的改动。";
  const vis = visibleReasoningText(raw);
  assert.match(vis, /用户问有哪些未提交的改动/);
  assert.equal(vis.includes("AQEA"), false);
  assert.equal(vis.includes("signature"), false);
});

test("bare ciphertext paints nothing", () => {
  const raw =
    "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDxKPiNBnLBNry-_0wFAUfhbZ8akGc_JDMPVT78QAPrltzmOdjrXDkEdQgh5Grt";
  assert.equal(visibleReasoningText(raw), "");
});

test("plain summary unchanged", () => {
  assert.equal(visibleReasoningText("先看 store。"), "先看 store。");
});

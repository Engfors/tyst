import assert from "node:assert/strict";
import { test } from "node:test";
import { implicitSave, namingDecision, titleToSave } from "./naming-action.ts";

test("Enter or Save with an empty name saves the default title", () => {
  assert.equal(titleToSave(""), null);
  assert.equal(titleToSave("   "), null);
  assert.equal(titleToSave("  Sync  "), "Sync");
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "input", title: "" }), {
    type: "save",
    title: null,
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "input", title: "  " }), {
    type: "save",
    title: null,
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "input", title: " Sync " }), {
    type: "save",
    title: "Sync",
  });
});

test("Escape on the name prompt asks before discarding", () => {
  for (const target of ["input", "keep", "discard", "other"] as const) {
    assert.deepEqual(namingDecision({ confirming: false, key: "Escape", target, title: "Notes" }), {
      type: "ask-discard",
    });
  }
});

test("Enter away from Discard, and a second Escape, keep the recording", () => {
  for (const target of ["input", "keep", "other"] as const) {
    assert.deepEqual(namingDecision({ confirming: true, key: "Enter", target, title: "Notes" }), {
      type: "keep",
    });
  }
  for (const target of ["input", "keep", "discard", "other"] as const) {
    assert.deepEqual(namingDecision({ confirming: true, key: "Escape", target, title: "Notes" }), {
      type: "keep",
    });
  }
});

test("Enter on the Discard button is the only key that discards", () => {
  assert.deepEqual(namingDecision({ confirming: true, key: "Enter", target: "discard", title: "Notes" }), {
    type: "discard",
  });
  assert.deepEqual(namingDecision({ confirming: true, key: " ", target: "discard", title: "Notes" }), {
    type: "ignore",
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "discard", title: "Notes" }), {
    type: "ignore",
  });
});

test("cancelling the confirmation does not save, and click-away still does", () => {
  assert.deepEqual(implicitSave(true, "Notes"), { type: "ignore" });
  assert.deepEqual(implicitSave(true, ""), { type: "ignore" });
  assert.deepEqual(implicitSave(false, ""), { type: "save", title: null });
  assert.deepEqual(implicitSave(false, "  Notes  "), { type: "save", title: "Notes" });
});

test("other keys leave the prompt alone", () => {
  assert.deepEqual(namingDecision({ confirming: false, key: "Tab", target: "input", title: "Notes" }), {
    type: "ignore",
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "other", title: "Notes" }), {
    type: "ignore",
  });
  assert.deepEqual(namingDecision({ confirming: true, key: "Tab", target: "keep", title: "Notes" }), {
    type: "ignore",
  });
});

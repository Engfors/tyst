import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { implicitSave, namingDecision, titleToSave } from "./naming-action.ts";

test("Enter or Save with an empty name saves the default title", () => {
  assert.equal(titleToSave(""), null);
  assert.equal(titleToSave("   "), null);
  assert.equal(titleToSave("  Sync  "), "Sync");
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "input", title: "" }), {
    type: "save",
    title: null,
    preventDefault: true,
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "input", title: "  " }), {
    type: "save",
    title: null,
    preventDefault: true,
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "input", title: " Sync " }), {
    type: "save",
    title: "Sync",
    preventDefault: true,
  });
});

test("Escape on the name prompt asks before discarding", () => {
  for (const target of ["input", "keep", "discard", "other"] as const) {
    assert.deepEqual(namingDecision({ confirming: false, key: "Escape", target, title: "Notes" }), {
      type: "ask-discard",
      preventDefault: true,
    });
  }
});

test("Enter away from Discard, and a second Escape, keep the recording", () => {
  for (const target of ["input", "keep", "other"] as const) {
    assert.deepEqual(namingDecision({ confirming: true, key: "Enter", target, title: "Notes" }), {
      type: "keep",
      preventDefault: true,
    });
  }
  for (const target of ["input", "keep", "discard", "other"] as const) {
    assert.deepEqual(namingDecision({ confirming: true, key: "Escape", target, title: "Notes" }), {
      type: "keep",
      preventDefault: true,
    });
  }
});

test("Enter on Discard discards; Space does not", () => {
  assert.deepEqual(namingDecision({ confirming: true, key: "Enter", target: "discard", title: "Notes" }), {
    type: "discard",
    preventDefault: true,
  });
  // Space is not its own Discard action. It only cancels the key, which is what stops the
  // focused button from being clicked. Space on Keep is the safe choice.
  assert.deepEqual(namingDecision({ confirming: true, key: " ", target: "discard", title: "Notes" }), {
    type: "ignore",
    preventDefault: true,
  });
  assert.deepEqual(namingDecision({ confirming: true, key: " ", target: "keep", title: "Notes" }), {
    type: "keep",
    preventDefault: true,
  });
  assert.deepEqual(namingDecision({ confirming: true, key: " ", target: "other", title: "Notes" }), {
    type: "ignore",
    preventDefault: true,
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "discard", title: "Notes" }), {
    type: "ignore",
    preventDefault: false,
  });
});

test("the meeting window applies preventDefault from the key policy", () => {
  const spaceOnDiscard = namingDecision({ confirming: true, key: " ", target: "discard", title: "Notes" });
  assert.equal(spaceOnDiscard.type, "ignore");
  assert.equal(spaceOnDiscard.preventDefault, true);
  const svelte = readFileSync(new URL("./Meeting.svelte", import.meta.url), "utf8");
  // Removing this line lets Space click the focused Discard button.
  assert.match(svelte, /if \(decision\.preventDefault\) e\.preventDefault\(\)/);
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
    preventDefault: false,
  });
  assert.deepEqual(namingDecision({ confirming: false, key: "Enter", target: "other", title: "Notes" }), {
    type: "ignore",
    preventDefault: false,
  });
  assert.deepEqual(namingDecision({ confirming: true, key: "Tab", target: "keep", title: "Notes" }), {
    type: "ignore",
    preventDefault: false,
  });
});

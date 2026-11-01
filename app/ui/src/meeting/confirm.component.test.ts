import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { Snapshot } from "../lib/api";
import Meeting from "./Meeting.svelte";

const calls = vi.hoisted(() => ({
  appState: vi.fn(),
  save: vi.fn(),
  discard: vi.fn(),
  hide: vi.fn(),
  preview: vi.fn(),
  giveBackFocus: vi.fn(async () => {}),
  compact: vi.fn(async () => {}),
  setLanguage: vi.fn(async () => {}),
  start: vi.fn(async () => {}),
  stop: vi.fn(async () => {}),
  togglePause: vi.fn(async () => {}),
  openPath: vi.fn(async () => {}),
  revealPath: vi.fn(async () => {}),
}));

vi.mock("../lib/api", () => ({
  STALE_PROMPT: "stale",
  api: {
    appState: calls.appState,
    save: calls.save,
    discard: calls.discard,
    hide: calls.hide,
    preview: calls.preview,
    giveBackFocus: calls.giveBackFocus,
    compact: calls.compact,
    setLanguage: calls.setLanguage,
    start: calls.start,
    stop: calls.stop,
    togglePause: calls.togglePause,
    openPath: calls.openPath,
    revealPath: calls.revealPath,
  },
  onMeeting: vi.fn(async () => () => {}),
  errorText: (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e)),
  formatElapsed: () => "0:00",
}));

function naming(seconds: number): Snapshot {
  return {
    phase: "naming",
    elapsed_ms: 0,
    language: "auto",
    labels: { me: "Me", others: "Others" },
    compact: false,
    segments: [],
    partials: [],
    naming: {
      placeholder: "2026-11-01 0800 Meeting",
      path_preview: "/tmp/2026-11-01 0800 Meeting.md",
      seconds,
      token: 7,
    },
    warning: null,
    channels: [],
  };
}

async function shown(seconds = 30) {
  calls.appState.mockResolvedValue(naming(seconds));
  calls.save.mockResolvedValue("/tmp/note.md");
  calls.discard.mockResolvedValue(undefined);
  calls.preview.mockResolvedValue("/tmp/2026-11-01 0800 Meeting.md");
  const view = render(Meeting);
  const input = await screen.findByLabelText("Name this meeting");
  return { ...view, input: input as HTMLInputElement };
}

describe("discard confirmation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  test("Escape then Enter keeps and leaves the typed name", async () => {
    const user = userEvent.setup();
    const { input } = await shown();
    await user.type(input, "Standup");
    await user.keyboard("{Escape}");
    expect(await screen.findByRole("alertdialog", { name: "Discard this recording?" })).toBeTruthy();
    expect(document.activeElement).toHaveProperty("textContent", expect.stringContaining("Keep"));
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByLabelText("Name this meeting")).toHaveProperty("value", "Standup");
    expect(calls.save).not.toHaveBeenCalled();
    expect(calls.discard).not.toHaveBeenCalled();
  });

  test("Escape Escape keeps and leaves the typed name", async () => {
    const user = userEvent.setup();
    const { input } = await shown();
    await user.type(input, "Standup");
    await user.keyboard("{Escape}");
    await screen.findByRole("alertdialog");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByLabelText("Name this meeting")).toHaveProperty("value", "Standup");
    expect(calls.discard).not.toHaveBeenCalled();
    expect(calls.save).not.toHaveBeenCalled();
  });

  test("Tab to Discard then Enter discards and does not save a note", async () => {
    const user = userEvent.setup();
    const { input } = await shown();
    await user.type(input, "Standup");
    await user.keyboard("{Escape}");
    await screen.findByRole("alertdialog");
    await user.tab();
    expect(document.activeElement).toHaveProperty("textContent", expect.stringContaining("Discard"));
    await user.keyboard("{Enter}");
    expect(calls.discard).toHaveBeenCalledTimes(1);
    expect(calls.discard).toHaveBeenCalledWith(7);
    expect(calls.save).not.toHaveBeenCalled();
  });

  test("a naming timeout while the confirm dialog is open saves under Meeting", async () => {
    vi.useFakeTimers();
    calls.appState.mockResolvedValue(naming(1));
    calls.save.mockResolvedValue("/tmp/note.md");
    render(Meeting);
    const input = await screen.findByLabelText("Name this meeting");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await screen.findByRole("alertdialog");
    // Escape extends a 1s prompt to 10s. An empty name is the default title Meeting.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(calls.save).toHaveBeenCalledTimes(1);
    expect(calls.save).toHaveBeenCalledWith(null, 7);
    expect(calls.discard).not.toHaveBeenCalled();
  });

  test("a stale discard closes the dialog and keeps the typed name", async () => {
    const user = userEvent.setup();
    const { input } = await shown();
    await user.type(input, "Standup");
    calls.discard.mockRejectedValue("stale");
    await user.keyboard("{Escape}");
    await screen.findByRole("alertdialog");
    await user.tab();
    await user.keyboard("{Enter}");
    expect(calls.discard).toHaveBeenCalledWith(7);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByLabelText("Name this meeting")).toHaveProperty("value", "Standup");
    expect(calls.save).not.toHaveBeenCalled();
  });
});

<script lang="ts">
  // Dictation pill (SPEC 8.4), at the bottom centre of the screen. Listening: a small waveform
  // and the live text. Done (preview mode): the final text, editable, with
  // Enter paste · Ctrl+C copy · Esc discard · Tab language. Direct mode flashes "Pasted" with a
  // Copy button. The window is transparent; the pill sits at its bottom edge.
  import { onDestroy, onMount, tick } from "svelte";
  import { api, errorText, onDictation, type LanguageMode, type PillState } from "../lib/api";

  let st = $state<PillState | null>(null);
  let text = $state("");
  let partial = $state("");
  let draft = $state("");
  let levels = $state<number[]>([0, 0, 0, 0, 0]);
  let error = $state<string | null>(null);
  let editor: HTMLTextAreaElement | undefined = $state();
  let liveBox: HTMLElement | undefined = $state();
  let unlisten: (() => void) | undefined;

  const phase = $derived(st?.phase ?? "idle");
  const listening = $derived(phase === "starting" || phase === "listening");
  const busy = $derived(phase === "finishing" || phase === "redecoding" || phase === "pasting");
  const mac = $derived(st?.platform === "macos");
  const mod = $derived(mac ? "⌘" : "Ctrl+");
  const langLabel: Record<LanguageMode, string> = { auto: "Auto", sv: "SV", en: "EN" };

  async function apply(s: PillState) {
    const was = st?.phase;
    st = s;
    text = s.text;
    partial = s.partial;
    if (s.phase === "preview" && was !== "preview") {
      draft = s.text;
      await tick();
      editor?.focus();
      editor?.setSelectionRange(draft.length, draft.length);
      fit();
    }
    if (s.phase === "starting") levels = [0, 0, 0, 0, 0];
  }

  function fit() {
    if (!editor) return;
    editor.style.height = "auto";
    editor.style.height = `${Math.min(editor.scrollHeight, 4 * 20 + 12)}px`;
  }

  async function act(f: () => Promise<void>) {
    try {
      error = null;
      await f();
    } catch (e) {
      error = errorText(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      act(listening ? api.dictationCancel : api.dictationDiscard);
    } else if (e.key === "Tab") {
      e.preventDefault();
      if (listening || phase === "preview") act(api.dictationCycleLanguage);
    } else if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (listening) act(api.dictationStop);
      else if (phase === "preview") act(() => api.dictationPaste(draft));
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c" && phase === "preview") {
      const sel = editor && editor.selectionStart !== editor.selectionEnd;
      if (!sel) {
        e.preventDefault();
        act(() => api.dictationCopy(draft));
      }
    }
  }

  $effect(() => {
    // Keep the newest words in view while listening.
    void text;
    void partial;
    if (liveBox) liveBox.scrollTop = liveBox.scrollHeight;
  });

  onMount(async () => {
    unlisten = await onDictation((e) => {
      if (e.type === "state") apply(e);
      else if (e.type === "text") {
        text = e.text;
        partial = e.partial;
      } else if (e.type === "level") {
        levels = [...levels.slice(1), Math.min(1, e.rms * 9)];
      }
    });
    const s = await api.dictationState();
    if (s) apply(s);
  });

  onDestroy(() => unlisten?.());
</script>

<svelte:window onkeydown={onKey} />

<div class="stage">
  {#if st && phase !== "idle"}
    <div class="pill" class:wide={phase === "preview" || phase === "redecoding"} role="status" aria-live="polite">
      {#if listening}
        <div class="row">
          <div class="wave" aria-hidden="true">
            {#each levels as l, i (i)}
              <span style={`height: ${4 + l * 18}px`}></span>
            {/each}
          </div>
          <div class="live" bind:this={liveBox}>
            {#if !text && !partial}
              <span class="placeholder">{phase === "starting" ? "Starting…" : "Listening…"}</span>
            {:else}
              <span>{text}</span>
              {#if partial}<span class="partial">{text ? " " : ""}{partial}</span>{/if}
            {/if}
          </div>
          <button class="lang" title="Language (Tab)" onclick={() => act(api.dictationCycleLanguage)}>{langLabel[st.language]}</button>
        </div>
        <div class="hints"><kbd>⏎</kbd> Done <kbd>Esc</kbd> Cancel <kbd>Tab</kbd> Language</div>
      {:else if phase === "preview" || phase === "redecoding"}
        <textarea
          bind:this={editor}
          bind:value={draft}
          oninput={() => (fit(), api.dictationEdit(draft))}
          rows="1"
          spellcheck="false"
          disabled={phase === "redecoding"}
          aria-label="Dictated text"
        ></textarea>
        <div class="hints">
          {#if phase === "redecoding"}
            <span>Decoding again in {langLabel[st.language]}…</span>
          {:else}
            <kbd>⏎</kbd> Paste{st.terminal ? ` (${mod}Shift+V)` : ""} <kbd>{mod}C</kbd> Copy <kbd>Esc</kbd> Discard
            <kbd>Tab</kbd> {langLabel[st.language]}
          {/if}
        </div>
      {:else if busy}
        <div class="row">
          <div class="live dim">{text || "…"}</div>
          <span class="spinner" aria-label="Working"></span>
        </div>
      {:else if phase === "pasted"}
        <div class="row">
          <span class="ok">✓</span>
          <span class="flash">Pasted</span>
          <span class="spacer"></span>
          <button onclick={() => act(() => api.dictationCopy(text))}>Copy</button>
        </div>
      {:else if phase === "message"}
        <div class="row"><span class="flash">{st.message}</span></div>
      {/if}
      {#if error}<div class="error">{error}</div>{/if}
    </div>
  {/if}
</div>

<style>
  :global(body.pill) {
    background: transparent;
    overflow: hidden;
  }

  .stage {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    padding: 6px 10px 10px;
  }

  .pill {
    width: 440px;
    max-width: 100%;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 22px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.22);
    padding: 9px 14px 7px;
    animation: rise var(--motion) ease-out;
  }

  .pill.wide {
    width: 540px;
    border-radius: 16px;
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 26px;
  }

  .wave {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 24px;
    flex: none;
  }

  .wave span {
    width: 3px;
    border-radius: 2px;
    background: var(--accent);
    transition: height 80ms linear;
  }

  .live {
    flex: 1;
    min-width: 0;
    max-height: 2.9em;
    overflow: hidden;
    line-height: 1.45;
    font-size: 14px;
  }

  .live.dim {
    color: var(--muted);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .placeholder,
  .partial {
    color: var(--muted);
  }

  .partial {
    font-style: italic;
  }

  .lang {
    flex: none;
    font-size: 11px;
    font-weight: 600;
    padding: 1px 7px;
    border-radius: 10px;
    color: var(--muted);
  }

  textarea {
    display: block;
    width: 100%;
    resize: none;
    border: none;
    outline: none;
    background: transparent;
    color: inherit;
    font: inherit;
    font-size: 14px;
    line-height: 20px;
    padding: 2px 0 4px;
    max-height: 92px;
    overflow-y: auto;
  }

  textarea:disabled {
    color: var(--muted);
  }

  .hints {
    margin-top: 3px;
    font-size: 11px;
    color: var(--faint);
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
  }

  kbd {
    font: inherit;
    font-size: 10px;
    padding: 0 4px;
    border: 1px solid var(--line);
    border-radius: 4px;
    color: var(--muted);
  }

  kbd:not(:first-child) {
    margin-left: 6px;
  }

  .ok {
    color: var(--ok);
    font-weight: 700;
  }

  .flash {
    font-size: 14px;
  }

  .spacer {
    flex: 1;
  }

  .spinner {
    width: 12px;
    height: 12px;
    flex: none;
    border-radius: 50%;
    border: 2px solid var(--line);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation-duration: 2.4s;
    }
  }

  .error {
    color: var(--warn);
    font-size: 12px;
    margin-top: 3px;
  }
</style>

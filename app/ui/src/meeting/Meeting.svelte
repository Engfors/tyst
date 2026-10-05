<script lang="ts">
  // Floating meeting window (SPEC 8.3): header with recording state, level meters, language,
  // pause and stop; the live transcript with scrollback; the name prompt after stopping; the
  // saved toast. The window never asks for focus except for the name field.
  import { onDestroy, onMount, tick } from "svelte";
  import {
    api,
    errorText,
    formatElapsed,
    onMeeting,
    type Channel,
    type Lang,
    type LanguageMode,
    type MeetingEvent,
    type Partial,
    type Segment,
    type Snapshot,
  } from "../lib/api";

  let snap = $state<Snapshot | null>(null);
  let segments = $state<Segment[]>([]);
  let partials = $state<Partial[]>([]);
  let levels = $state<Record<Channel, number>>({ me: 0, others: 0 });
  let warning = $state<string | null>(null);
  let elapsed = $state(0);
  let startedAt = 0;
  let saved = $state<string | null>(null);
  let title = $state("");
  let preview = $state<string | null>(null);
  let countdown = $state(0);
  let saving = false;
  // A failed save, shown in the name prompt: the prompt covers the warning bar.
  let saveError = $state<string | null>(null);
  let followLatest = $state(true);
  let body: HTMLElement | undefined = $state();
  let titleInput: HTMLInputElement | undefined = $state();
  let timers: ReturnType<typeof setInterval>[] = [];
  let unlisten: (() => void) | undefined;
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  const phase = $derived(snap?.phase ?? "idle");
  const recording = $derived(phase === "recording" || phase === "paused" || phase === "starting");
  const compact = $derived(snap?.compact ?? false);

  type Turn = { key: string; channel: Channel; lang: Lang | null; text: string; partial: string; showLang: boolean };

  // Consecutive segments of one channel become one turn (as in the Markdown file); the open
  // partials are appended to the turn they continue, or start a new one.
  const turns = $derived.by(() => {
    const items: { channel: Channel; start: number; text: string; lang: Lang | null; partial: boolean; id: number }[] =
      segments.map((s) => ({ channel: s.channel, start: s.start_ms, text: s.text, lang: s.lang, partial: false, id: s.id }));
    const sorted = items.sort((a, b) => a.start - b.start || a.id - b.id);
    for (const p of partials) {
      sorted.push({ channel: p.channel, start: Number.MAX_SAFE_INTEGER, text: p.text, lang: null, partial: true, id: p.segment_id });
    }
    const out: Turn[] = [];
    let prevLang: Lang | null = null;
    for (const it of sorted) {
      if (!it.text.trim()) continue;
      const last = out[out.length - 1];
      if (last && last.channel === it.channel) {
        if (it.partial) last.partial = (last.partial ? last.partial + " " : "") + it.text;
        else last.text = (last.text ? last.text + " " : "") + it.text;
        if (!last.lang && it.lang) last.lang = it.lang;
        continue;
      }
      const lang = it.lang;
      const showLang = !!lang && prevLang !== null && lang !== prevLang;
      if (lang) prevLang = lang;
      out.push({
        key: `${it.channel}-${it.id}`,
        channel: it.channel,
        lang,
        text: it.partial ? "" : it.text,
        partial: it.partial ? it.text : "",
        showLang,
      });
    }
    return out;
  });

  const latestLine = $derived.by(() => {
    const t = turns[turns.length - 1];
    if (!t) return "";
    const all = [t.text, t.partial].filter(Boolean).join(" ");
    return all.length > 90 ? "…" + all.slice(-90) : all;
  });

  function fileName(path: string | null): string {
    return path ? (path.split(/[\\/]/).pop() ?? path) : "";
  }

  function label(ch: Channel): string {
    return ch === "me" ? (snap?.labels.me ?? "Me") : (snap?.labels.others ?? "Others");
  }

  function apply(s: Snapshot) {
    const wasNaming = snap?.phase === "naming";
    snap = s;
    segments = s.segments;
    partials = s.partials;
    warning = s.warning;
    startedAt = Date.now() - s.elapsed_ms;
    elapsed = s.elapsed_ms;
    if (s.phase === "starting") saved = null;
    if (s.phase === "naming" && !wasNaming && s.naming) startNaming(s.naming.seconds);
  }

  async function startNaming(seconds: number) {
    title = "";
    preview = snap?.naming?.path_preview ?? null;
    countdown = seconds;
    saving = false;
    saveError = null;
    await tick();
    titleInput?.focus();
  }

  async function handle(e: MeetingEvent) {
    switch (e.type) {
      case "state": {
        const { type: _, ...s } = e;
        apply(s);
        break;
      }
      case "partial":
        partials = [...partials.filter((p) => p.channel !== e.channel), { channel: e.channel, segment_id: e.segment_id, text: e.text }];
        break;
      case "final":
        partials = partials.filter((p) => !(p.channel === e.channel && p.segment_id === e.id));
        segments = [...segments, { id: e.id, channel: e.channel, lang: e.lang, start_ms: e.start_ms, text: e.text }];
        break;
      case "dropped":
        partials = partials.filter((p) => !(p.channel === e.channel && p.segment_id === e.segment_id));
        break;
      case "level":
        // Fast attack, slow release, on a perceptual (dB-ish) scale.
        levels[e.channel] = Math.max(Math.min(1, Math.max(0, (20 * Math.log10(e.rms + 1e-6) + 60) / 50)), levels[e.channel] * 0.6);
        break;
      case "warning":
        warning = e.message;
        break;
      case "saved":
        showSaved(e.path);
        break;
    }
    if (followLatest && body) {
      await tick();
      body.scrollTop = body.scrollHeight;
    }
  }

  function showSaved(path: string) {
    saved = path;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      saved = null;
      if (snap?.phase === "idle") api.hide();
    }, 6000);
  }

  function onScroll() {
    if (!body) return;
    followLatest = body.scrollHeight - body.scrollTop - body.clientHeight < 24;
  }

  function jumpToLatest() {
    followLatest = true;
    if (body) body.scrollTop = body.scrollHeight;
  }

  const order: LanguageMode[] = ["auto", "sv", "en"];
  function cycleLanguage() {
    const cur = snap?.language ?? "auto";
    api.setLanguage(order[(order.indexOf(cur) + 1) % order.length]).catch(fail);
  }

  function fail(e: unknown) {
    warning = errorText(e);
  }

  async function save(withTitle: boolean) {
    if (saving || snap?.phase !== "naming") return;
    saving = true;
    try {
      await api.save(withTitle && title.trim() ? title.trim() : null);
      saveError = null;
    } catch (e) {
      saving = false;
      saveError = `Could not save: ${errorText(e)}`;
      titleInput?.focus();
    }
  }

  let previewTimer: ReturnType<typeof setTimeout> | undefined;
  function onTitleInput() {
    clearTimeout(previewTimer);
    countdown = Math.max(countdown, 10); // typing keeps the prompt open a little longer
    previewTimer = setTimeout(async () => {
      preview = await api.preview(title.trim() || null);
    }, 150);
  }

  function onTitleKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      save(true);
    } else if (e.key === "Escape") {
      e.preventDefault();
      save(false);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement) return;
    if (e.key === "Escape" && phase === "idle") api.hide();
  }

  onMount(async () => {
    unlisten = await onMeeting(handle);
    apply(await api.appState());
    timers.push(
      setInterval(() => {
        if (phase === "recording") elapsed = Date.now() - startedAt;
        levels = { me: levels.me * 0.85, others: levels.others * 0.85 };
      }, 100),
      setInterval(() => {
        if (snap?.phase === "naming" && countdown > 0) {
          countdown -= 1;
          if (countdown === 0) save(!!title.trim());
        }
      }, 1000),
    );
  });

  onDestroy(() => {
    unlisten?.();
    timers.forEach(clearInterval);
  });
</script>

<svelte:window onkeydown={onKey} />

<main class:compact>
  <header data-tauri-drag-region>
    <div class="status" data-tauri-drag-region>
      {#if phase === "recording"}
        <span class="dot rec" title="Recording"></span>
      {:else if phase === "paused"}
        <span class="dot paused" title="Paused"></span>
      {:else if phase === "starting" || phase === "stopping"}
        <span class="dot starting" title={phase === "starting" ? "Starting" : "Stopping"}></span>
      {:else}
        <span class="dot idle"></span>
      {/if}
      <span class="time mono" data-tauri-drag-region>
        {#if phase === "starting"}Loading…{:else if phase === "stopping"}Stopping…{:else if recording}{formatElapsed(elapsed)}{:else if phase === "naming"}Stopped{:else}Tyst{/if}
      </span>
    </div>

    {#if compact}
      <div class="line" data-tauri-drag-region><span dir="ltr">{latestLine}</span></div>
    {:else}
      <div class="meters" data-tauri-drag-region>
        {#each snap?.channels ?? [] as ch}
          <span class="meter" title={label(ch)}>
            <span class="meter-label">{label(ch)}</span>
            <span class="meter-bar"><span class={`meter-fill ${ch}`} style:width={`${Math.round(levels[ch] * 100)}%`}></span></span>
          </span>
        {/each}
      </div>
    {/if}

    <div class="actions">
      {#if recording || phase === "idle"}
        <button class="badge" title="Language: Auto → Svenska → English" onclick={cycleLanguage}>
          {(snap?.language ?? "auto") === "auto" ? "Auto" : (snap?.language ?? "").toUpperCase()}
        </button>
      {/if}
      {#if phase === "recording" || phase === "paused"}
        <button class="icon" title={phase === "paused" ? "Resume" : "Pause"} onclick={() => api.togglePause().catch(fail)}>
          {#if phase === "paused"}
            <svg viewBox="0 0 16 16"><path d="M5 3.5v9l7-4.5z" /></svg>
          {:else}
            <svg viewBox="0 0 16 16"><rect x="4" y="3.5" width="3" height="9" rx="1" /><rect x="9" y="3.5" width="3" height="9" rx="1" /></svg>
          {/if}
        </button>
        <button class="icon stop" title="Stop" onclick={() => api.stop().catch(fail)}>
          <svg viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="1.5" /></svg>
        </button>
      {:else if phase === "starting"}
        <button class="icon stop" title="Cancel" onclick={() => api.stop().catch(fail)}>
          <svg viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="1.5" /></svg>
        </button>
      {:else if phase === "idle"}
        <button class="icon start" title="Start meeting transcription" onclick={() => api.start().catch(fail)}>
          <svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="4.5" /></svg>
        </button>
      {/if}
      <button class="icon" title={compact ? "Expand" : "Compact"} onclick={() => api.compact(!compact).then(() => snap && (snap.compact = !compact))}>
        {#if compact}
          <svg viewBox="0 0 16 16"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.6" /></svg>
        {:else}
          <svg viewBox="0 0 16 16"><path d="M4 10l4-4 4 4" fill="none" stroke="currentColor" stroke-width="1.6" /></svg>
        {/if}
      </button>
      <button class="icon" title="Hide (recording continues)" onclick={() => api.hide()}>
        <svg viewBox="0 0 16 16"><path d="M4.5 4.5l7 7M11.5 4.5l-7 7" fill="none" stroke="currentColor" stroke-width="1.6" /></svg>
      </button>
    </div>
  </header>

  {#if !compact}
    <section class="body" bind:this={body} onscroll={onScroll}>
      {#each turns as t (t.key)}
        <p class={`turn ${t.channel}`}>
          <span class="who">{label(t.channel)}</span>
          {#if t.showLang && t.lang}<span class="lang">{t.lang.toUpperCase()}</span>{/if}
          <span class="text">{t.text}</span>
          {#if t.partial}<span class="partial">{t.partial}</span>{/if}
        </p>
      {:else}
        <p class="empty muted">
          {#if phase === "starting"}Loading models…
          {:else if phase === "stopping"}Finishing the transcript…
          {:else if recording}Listening…
          {:else if phase === "idle"}Start a meeting from here or the tray icon.
          {/if}
        </p>
      {/each}
    </section>
    {#if !followLatest}
      <button class="latest" onclick={jumpToLatest}>↓ Latest</button>
    {/if}
  {/if}

  {#if phase === "naming" && snap?.naming}
    <form class="naming" onsubmit={(e) => { e.preventDefault(); save(true); }}>
      <label for="title">Name this meeting</label>
      <div class="row">
        <input
          id="title"
          type="text"
          bind:this={titleInput}
          bind:value={title}
          placeholder={snap.naming.placeholder}
          autocomplete="off"
          spellcheck="false"
          oninput={onTitleInput}
          onkeydown={onTitleKey}
          onblur={() => setTimeout(() => save(!!title.trim()), 150)}
        />
        <button class="primary" type="submit">Save</button>
      </div>
      {#if saveError}
        <div class="save-error" role="alert">{saveError}</div>
      {/if}
      <div class="hint muted">
        <span class="path mono" title={preview ?? ""}>{fileName(preview)}</span>
        <span class="count">Esc: save as is · {countdown}s</span>
      </div>
    </form>
  {/if}

  {#if saved}
    <div class="toast" role="status">
      <span>Saved</span>
      <button onclick={() => saved && api.openPath(saved).catch(fail)}>Open</button>
      <button onclick={() => saved && api.revealPath(saved).catch(fail)}>Show in folder</button>
    </div>
  {/if}

  {#if warning && !compact}
    <div class="warning" role="alert">
      <span>{warning}</span>
      <button class="link" onclick={() => (warning = null)}>Dismiss</button>
    </div>
  {/if}
</main>

<style>
  :global(body.meeting) {
    background: transparent;
    overflow: hidden;
  }

  main {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 42px;
    flex: 0 0 42px;
    padding: 0 8px 0 12px;
    border-bottom: 1px solid var(--line);
  }

  main.compact header {
    border-bottom: none;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: none;
  }

  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--faint);
  }

  .dot.rec {
    background: var(--rec);
    animation: pulse 1.6s ease-in-out infinite;
  }

  .dot.paused {
    background: var(--pause);
  }

  .dot.starting {
    background: var(--rec);
    opacity: 0.5;
  }

  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .dot.rec {
      animation: none;
    }
  }

  .time {
    color: var(--muted);
    min-width: 46px;
  }

  .meters {
    display: flex;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }

  .meter {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
  }

  .meter-label {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 70px;
  }

  .meter-bar {
    width: 38px;
    height: 4px;
    border-radius: 2px;
    background: var(--line);
    overflow: hidden;
    flex: none;
  }

  .meter-fill {
    display: block;
    height: 100%;
    border-radius: 2px;
  }

  .meter-fill.me {
    background: var(--me);
  }

  .meter-fill.others {
    background: var(--others);
  }

  .line {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    /* Clip the start, so the newest words stay visible. */
    direction: rtl;
    text-align: left;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
  }

  button.icon {
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    background: transparent;
    display: grid;
    place-items: center;
    color: var(--muted);
  }

  button.icon:hover {
    background: var(--line);
    color: var(--fg);
  }

  button.icon svg {
    width: 15px;
    height: 15px;
    fill: currentColor;
  }

  button.icon.stop,
  button.icon.start {
    color: var(--rec);
  }

  button.badge {
    font-size: 11px;
    font-weight: 600;
    padding: 1px 7px;
    border-radius: 6px;
    color: var(--muted);
    background: transparent;
    margin-right: 2px;
  }

  .body {
    flex: 1;
    overflow-y: auto;
    padding: 8px 14px 12px;
    user-select: text;
    -webkit-user-select: text;
  }

  .turn {
    margin: 0 0 8px;
  }

  .who {
    font-weight: 600;
    margin-right: 6px;
  }

  .turn.me .who {
    color: var(--me);
  }

  .turn.others .who {
    color: var(--others);
  }

  .lang {
    font-size: 10px;
    font-weight: 600;
    color: var(--muted);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0 4px;
    margin-right: 6px;
  }

  .partial {
    color: var(--muted);
    font-style: italic;
  }

  .text + .partial {
    margin-left: 0.3em;
  }

  .empty {
    margin-top: 18px;
    text-align: center;
  }

  .latest {
    position: absolute;
    right: 12px;
    bottom: 12px;
    font-size: 12px;
    border-radius: 12px;
    padding: 2px 10px;
    background: var(--accent);
    color: var(--accent-fg);
    border: none;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  }

  .naming {
    position: absolute;
    inset: 42px 0 0 0;
    background: var(--bg);
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    animation: fade var(--motion) ease-out;
  }

  main.compact .naming {
    position: static;
    padding: 6px 10px 10px;
  }

  .naming label {
    font-weight: 600;
  }

  .naming .row {
    display: flex;
    gap: 6px;
  }

  .naming input {
    flex: 1;
    min-width: 0;
  }

  .save-error {
    font-size: 12px;
    color: var(--warn);
  }

  .hint {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    font-size: 11px;
  }

  .path {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .count {
    flex: none;
    white-space: nowrap;
  }

  .toast {
    position: absolute;
    left: 50%;
    bottom: 12px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px 6px 12px;
    border-radius: 10px;
    background: var(--fg);
    color: var(--bg);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
    animation: fade var(--motion) ease-out;
    white-space: nowrap;
  }

  main.compact .toast {
    position: static;
    transform: none;
    margin: 0 8px 8px;
  }

  .toast button {
    background: transparent;
    color: inherit;
    border-color: color-mix(in srgb, var(--bg), transparent 60%);
    padding: 2px 8px;
  }

  .warning {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    font-size: 12px;
    color: var(--warn);
    border-top: 1px solid var(--line);
  }

  .warning span {
    flex: 1;
  }

  button.link {
    border: none;
    background: transparent;
    color: var(--muted);
    padding: 0;
    font-size: 12px;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>

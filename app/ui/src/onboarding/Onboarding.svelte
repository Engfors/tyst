<script lang="ts">
  // First run (SPEC 8.5): welcome, transcripts folder, models, audio access, dictation
  // (shortcuts and paste access), preferences, test.
  import { onMount } from "svelte";
  import { api, errorText, type ConfigView, type DictationInfo } from "../lib/api";
  import Models from "../lib/Models.svelte";

  const steps = ["Welcome", "Folder", "Models", "Audio", "Dictation", "Preferences", "Test"] as const;
  let step = $state(0);
  let view = $state<ConfigView | null>(null);
  let modelsReady = $state(false);
  let error = $state<string | null>(null);

  type Check = { state: "idle" | "running" | "ok" | "quiet" | "failed"; detail: string };
  let mic = $state<Check>({ state: "idle", detail: "" });
  let system = $state<Check>({ state: "idle", detail: "" });
  let test = $state<Check>({ state: "idle", detail: "" });
  let info = $state<DictationInfo | null>(null);
  let paste = $state<Check>({ state: "idle", detail: "" });

  async function setupShortcuts() {
    await api.dictationSetupShortcuts().catch((e) => (error = errorText(e)));
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 1000));
      info = await api.dictationInfo().catch(() => null);
      if (info && !info.shortcuts.pending) break;
    }
  }

  async function allowPaste() {
    paste = { state: "running", detail: "" };
    try {
      await api.dictationSetupKeyboard();
      paste = { state: "ok", detail: "Allowed." };
    } catch (e) {
      paste = { state: "failed", detail: errorText(e) };
    }
  }

  const canNext = $derived(
    step === 1 ? !!view?.config.transcripts_dir : step === 2 ? modelsReady : true,
  );

  async function save() {
    if (view) await api.configSet($state.snapshot(view.config)).catch((e) => (error = errorText(e)));
  }

  async function chooseFolder() {
    if (!view) return;
    const p = await api.pickFolder(view.config.transcripts_dir ?? view.default_transcripts_dir);
    if (p) {
      view.config.transcripts_dir = p;
      await save();
    }
  }

  async function check(channel: "me" | "others", target: (c: Check) => void, seconds: number) {
    target({ state: "running", detail: "" });
    try {
      const r = await api.audioTest(channel, seconds);
      const heard = r.peak > 0.02;
      target({
        state: heard ? "ok" : "quiet",
        detail: r.text || (heard ? "Sound received." : "Nothing heard. Check the input device and its volume."),
      });
    } catch (e) {
      target({ state: "failed", detail: errorText(e) });
    }
  }

  async function finish() {
    await save();
    try {
      await api.onboardingFinish();
    } catch (e) {
      error = errorText(e);
    }
  }

  onMount(async () => {
    view = await api.configGet();
    if (!view.config.transcripts_dir) {
      view.config.transcripts_dir = view.default_transcripts_dir;
    }
  });
</script>

<div class="wrap">
  <ol class="steps">
    {#each steps as s, i}
      <li class:done={i < step} class:current={i === step}>{s}</li>
    {/each}
  </ol>

  <section>
    {#if step === 0}
      <h1>Welcome to Tyst</h1>
      <p>Tyst transcribes your meetings live, in Swedish and English, and saves them as Markdown files.</p>
      <p><b>Everything stays on this computer.</b> Speech recognition runs locally; no audio or text is sent anywhere. The only download is the speech model, once.</p>
      <p class="muted">Your microphone becomes “Me”, the computer's sound (Teams, Meet, Zoom) becomes “Others”.</p>
    {:else if step === 1 && view}
      <h1>Where should transcripts go?</h1>
      <p>Each meeting becomes a Markdown file in this folder.</p>
      <div class="row">
        <span class="mono path">{view.config.transcripts_dir}</span>
        <button onclick={chooseFolder}>Choose…</button>
      </div>
    {:else if step === 2}
      <h1>Download the speech model</h1>
      <Models compact ondone={() => (modelsReady = true)} />
      <p class="muted small">Already have the files? Pick their folder under Settings › Models later.</p>
    {:else if step === 3 && view}
      <h1>Audio access</h1>
      <p>Tyst needs the microphone{view.system_audio_supported ? " and the computer's sound" : ""}. Check that both come through:</p>
      <div class="check">
        <button onclick={() => check("me", (c) => (mic = c), 3)} disabled={mic.state === "running"}>Test microphone</button>
        <span class={`result ${mic.state}`}>{mic.state === "running" ? "Say something…" : mic.detail}</span>
      </div>
      {#if view.system_audio_supported}
        <div class="check">
          <button onclick={() => check("others", (c) => (system = c), 4)} disabled={system.state === "running"}>Test system audio</button>
          <span class={`result ${system.state}`}>{system.state === "running" ? "Play a video with speech now…" : system.detail}</span>
        </div>
      {/if}
      {#if view.platform === "macos"}
        <p class="muted small">macOS asks for Microphone access the first time. If it was denied, allow Tyst in System Settings › Privacy &amp; Security › Microphone.</p>
      {:else}
        <p class="muted small">On Linux, PipeWire normally needs no permission. On KDE, Tyst adds a window rule so the meeting window stays on top without taking focus.</p>
      {/if}
    {:else if step === 4 && view}
      <h1>Dictation</h1>
      <p>Dictate into any app: press <b>{view.platform === "macos" ? "Cmd+Å" : "Ctrl+Å"}</b>, speak, press it again (or hold it while you talk). The text shows in a small pill; Enter pastes it where you were typing.</p>
      {#if view.platform === "linux"}
        <div class="check">
          <button onclick={setupShortcuts}>Set up shortcuts</button>
          <span class="result">
            {#if info?.shortcuts.error}{info.shortcuts.error}
            {:else if info?.shortcuts.bound.length}{info.shortcuts.bound.map(([id, t]) => `${id === "dictate" ? "Dictate" : "Meeting"}: ${t || "not assigned"}`).join(" · ")}
            {:else if info?.shortcuts.pending}Confirm the keys in the dialog…{/if}
          </span>
        </div>
        <div class="check">
          <button onclick={allowPaste} disabled={paste.state === "running"}>Allow paste</button>
          <span class={`result ${paste.state}`}>{paste.state === "running" ? "Allow remote input in the dialog…" : paste.detail}</span>
        </div>
        <p class="muted small">Your desktop asks once for each: the shortcut keys, and permission for Tyst to press Ctrl+V for you. Change the keys later in System Settings › Keyboard › Shortcuts.</p>
      {:else if view.platform === "macos"}
        <p class="muted small">Pasting needs the Accessibility permission: System Settings › Privacy &amp; Security › Accessibility. macOS asks the first time Tyst pastes.</p>
      {/if}
    {:else if step === 5 && view}
      <h1>Preferences</h1>
      <label class="opt"><input type="checkbox" bind:checked={view.config.launch_at_login} onchange={save} /> Launch Tyst at login</label>
      <label class="opt"><input type="checkbox" bind:checked={view.config.meetings.system_audio} onchange={save} disabled={!view.system_audio_supported} /> Transcribe system audio as “Others”</label>
      <div class="tip">
        <b>Use headphones in meetings.</b> On speakers, the microphone also picks up the other participants, and their words appear twice.
      </div>
    {:else if step === 6}
      <h1>Try it</h1>
      <p>Click and say a sentence in Swedish or English.</p>
      <div class="check">
        <button class="primary" onclick={() => check("me", (c) => (test = c), 5)} disabled={test.state === "running"}>Say something</button>
        <span class={`result ${test.state}`}>{test.state === "running" ? "Listening for 5 seconds…" : ""}</span>
      </div>
      {#if test.detail && test.state !== "running"}<blockquote>{test.detail}</blockquote>{/if}
      <p class="muted">Start a meeting from the Tyst icon in the {view?.platform === "macos" ? "menu bar" : "system tray"}, or dictate with {view?.platform === "macos" ? "Cmd+Å" : "Ctrl+Å"} in any text field.</p>
    {/if}
    {#if error}<p class="error">{error}</p>{/if}
  </section>

  <footer>
    {#if step > 0}<button onclick={() => step--}>Back</button>{/if}
    <span class="spacer"></span>
    {#if step < steps.length - 1}
      <button class="primary" disabled={!canNext} onclick={() => step++}>Continue</button>
    {:else}
      <button class="primary" onclick={finish}>Done</button>
    {/if}
  </footer>
</div>

<style>
  .wrap {
    height: 100vh;
    display: flex;
    flex-direction: column;
    padding: 18px 26px;
  }
  .steps {
    display: flex;
    gap: 14px;
    list-style: none;
    padding: 0;
    margin: 0 0 14px;
    font-size: 12px;
    color: var(--faint);
  }
  .steps li.current {
    color: var(--fg);
    font-weight: 600;
  }
  .steps li.done {
    color: var(--muted);
  }
  section {
    flex: 1;
    overflow-y: auto;
  }
  h1 {
    font-size: 20px;
    margin: 6px 0 12px;
  }
  .row,
  .check {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 10px 0;
  }
  .path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .result.ok {
    color: var(--ok);
  }
  .result.quiet,
  .result.failed {
    color: var(--warn);
  }
  .opt {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 10px;
  }
  .tip {
    margin-top: 14px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: 10px;
  }
  blockquote {
    margin: 10px 0;
    padding: 10px 14px;
    border-left: 3px solid var(--accent);
    background: var(--field);
    border-radius: 6px;
  }
  footer {
    display: flex;
    gap: 8px;
    padding-top: 12px;
  }
  .spacer {
    flex: 1;
  }
  .small {
    font-size: 12px;
  }
  .error {
    color: var(--warn);
  }
</style>

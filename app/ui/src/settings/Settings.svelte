<script lang="ts">
  // Settings (SPEC 8.6): General, Dictation, Meetings, Models, Vocabulary, Updates, About.
  // Changes save at once.
  import { onDestroy, onMount } from "svelte";
  import { api, errorText, onUpdates, type ConfigView, type DictationInfo, type UpdateView } from "../lib/api";
  import Models from "../lib/Models.svelte";
  import Vocabulary from "../lib/Vocabulary.svelte";

  const tabs = [
    ["general", "General"],
    ["dictation", "Dictation"],
    ["meetings", "Meetings"],
    ["models", "Models"],
    ["vocabulary", "Vocabulary"],
    ["updates", "Updates"],
    ["about", "About"],
  ] as const;
  type Tab = (typeof tabs)[number][0];

  let tab = $state<Tab>("general");
  let view = $state<ConfigView | null>(null);
  let status = $state<string | null>(null);
  let info = $state<DictationInfo | null>(null);
  let terminals = $state("");
  let meetingApps = $state("");
  let infoTimer: ReturnType<typeof setInterval> | undefined;
  let updates = $state<UpdateView | null>(null);
  let unlistenUpdates: (() => void) | undefined;

  async function checkNow() {
    try {
      updates = await api.updatesCheck();
    } catch (e) {
      status = errorText(e);
    }
  }

  const shortcutNames: Record<string, string> = { dictate: "Dictate", "toggle-meeting": "Start/stop meeting" };

  async function refreshInfo() {
    info = await api.dictationInfo().catch(() => null);
  }

  function saveMeetingApps() {
    if (!view) return;
    view.config.meetings.detect_apps = meetingApps
      .split(/[\n,]/)
      .map((t) => t.trim())
      .filter(Boolean);
    save();
  }

  function saveTerminals() {
    if (!view) return;
    view.config.dictation.terminal_classes = terminals
      .split(/[\n,]/)
      .map((t) => t.trim())
      .filter(Boolean);
    save();
  }

  async function allowPaste() {
    status = "Waiting for the desktop's permission dialog…";
    try {
      await api.dictationSetupKeyboard();
      status = "Paste access granted.";
    } catch (e) {
      status = errorText(e);
    }
    refreshInfo();
  }

  async function rebind() {
    await api.dictationSetupShortcuts();
    status = "Confirm the shortcuts in the desktop's dialog if it asks.";
    setTimeout(refreshInfo, 1500);
  }

  function fromHash() {
    const h = location.hash.slice(1);
    if (tabs.some(([id]) => id === h)) tab = h as Tab;
  }

  async function save() {
    if (!view) return;
    try {
      await api.configSet($state.snapshot(view.config));
      status = null;
    } catch (e) {
      status = errorText(e);
    }
  }

  async function chooseFolder() {
    if (!view) return;
    const p = await api.pickFolder(view.config.transcripts_dir ?? view.default_transcripts_dir);
    if (p) {
      view.config.transcripts_dir = p;
      save();
    }
  }

  async function chooseModelsFolder() {
    if (!view) return;
    const p = await api.pickFolder(view.config.models_dir ?? view.default_models_dir);
    if (p) {
      view.config.models_dir = p;
      save();
    }
  }

  onMount(async () => {
    fromHash();
    view = await api.configGet();
    terminals = view.config.dictation.terminal_classes.join("\n");
    meetingApps = view.config.meetings.detect_apps.join("\n");
    refreshInfo();
    // The desktop confirms shortcuts asynchronously; keep the status current while open.
    infoTimer = setInterval(refreshInfo, 3000);
    updates = await api.updatesState();
    unlistenUpdates = await onUpdates((v) => (updates = v));
  });

  onDestroy(() => {
    clearInterval(infoTimer);
    unlistenUpdates?.();
  });
</script>

<svelte:window onhashchange={fromHash} />

<div class="layout">
  <nav>
    {#each tabs as [id, name]}
      <button class:active={tab === id} onclick={() => ((tab = id), history.replaceState(null, "", `#${id}`))}>{name}</button>
    {/each}
  </nav>

  <main>
    {#if view}
      {@const c = view.config}
      {#if tab === "general"}
        <h2>General</h2>
        <div class="field">
          <span class="name">Transcripts folder</span>
          <div class="row">
            <span class="mono path">{c.transcripts_dir ?? "Not chosen"}</span>
            <button onclick={chooseFolder}>Change…</button>
            <button onclick={() => api.openTranscripts().catch((e) => (status = errorText(e)))}>Open</button>
          </div>
        </div>
        <label class="check"><input type="checkbox" bind:checked={c.launch_at_login} onchange={save} /> Launch at login</label>
        <div class="field">
          <span class="name">Speaker labels</span>
          <div class="row">
            <label>You <input type="text" bind:value={c.labels.me} onchange={save} /></label>
            <label>Others <input type="text" bind:value={c.labels.others} onchange={save} /></label>
          </div>
        </div>
        <div class="field">
          <span class="name">Default language</span>
          <select bind:value={c.language} onchange={save}>
            <option value="auto">Auto (Swedish model, labels guessed)</option>
            <option value="sv">Svenska</option>
            <option value="en">English (needs the English model)</option>
          </select>
        </div>
        <div class="field">
          <span class="name">Recognition threads</span>
          <input type="number" min="1" max="16" bind:value={c.threads} onchange={save} />
          <span class="muted small">More threads: faster text, more CPU. Applies after the models reload.</span>
        </div>
        <div class="field">
          <span class="name">Models in memory</span>
          <select bind:value={c.models_idle_minutes} onchange={save}>
            <option value={0}>Keep loaded (fastest start)</option>
            <option value={15}>Unload after 15 idle minutes</option>
            <option value={30}>Unload after 30 idle minutes</option>
            <option value={60}>Unload after 1 idle hour</option>
          </select>
          <span class="muted small">Unloaded models free about 1 GB of memory and load again in a second or two when you next start a meeting or dictate.</span>
        </div>
      {:else if tab === "dictation"}
        {@const d = c.dictation}
        <h2>Dictation</h2>
        <label class="check"><input type="checkbox" bind:checked={d.enabled} onchange={save} /> Dictate with a shortcut</label>
        <div class="field">
          <span class="name">Shortcuts</span>
          {#if view.platform === "macos"}
            <div class="row">
              <label>Dictate <input type="text" class="mono" bind:value={d.shortcut} onchange={save} /></label>
              <label>Meeting <input type="text" class="mono" bind:value={d.meeting_shortcut} onchange={save} /></label>
            </div>
            <span class="muted small">Key names by position: <span class="mono">BracketLeft</span> is Å on a Swedish keyboard, <span class="mono">Super</span> is Cmd.</span>
          {:else}
            {#if info?.shortcuts.bound.length}
              <ul class="bound">
                {#each info.shortcuts.bound as [id, trigger]}
                  <li>{shortcutNames[id] ?? id}: <b>{trigger || "not assigned"}</b></li>
                {/each}
              </ul>
            {:else if info?.shortcuts.pending}
              <span class="muted">Waiting for the desktop…</span>
            {/if}
            <div class="row">
              <button onclick={rebind}>Set up again</button>
              <span class="muted small">Change the keys in System Settings › Keyboard › Shortcuts › Tyst.</span>
            </div>
          {/if}
          {#if info?.shortcuts.error}<span class="warn small">{info.shortcuts.error}</span>{/if}
        </div>
        <div class="field">
          <span class="name">Shortcut</span>
          <select bind:value={d.trigger} onchange={save}>
            <option value="hybrid">Tap to start and stop, hold to talk</option>
            <option value="toggle">Tap to start and stop</option>
            <option value="hold">Hold to talk</option>
          </select>
        </div>
        <div class="field">
          <span class="name">When you stop</span>
          <select bind:value={d.paste_mode} onchange={save}>
            <option value="preview">Show the text first (Enter pastes)</option>
            <option value="direct">Paste right away</option>
          </select>
        </div>
        <label class="check"><input type="checkbox" bind:checked={d.restore_clipboard} onchange={save} /> Put back what was on the clipboard after pasting</label>
        <div class="field">
          <span class="name">Language</span>
          <select bind:value={d.language} onchange={save}>
            <option value="auto">Auto</option>
            <option value="sv">Svenska</option>
            <option value="en" disabled={!info?.english_model}>English{info?.english_model ? "" : " (download the English model first)"}</option>
          </select>
          <span class="muted small">Tab in the pill switches language while dictating.</span>
        </div>
        {#if view.platform === "linux"}
          <div class="field">
            <span class="name">Paste access</span>
            <div class="row">
              <span class={info?.keyboard_granted ? "ok" : "muted"}>{info?.keyboard_granted ? "Allowed" : "Not allowed yet"}</span>
              <button onclick={allowPaste}>{info?.keyboard_granted ? "Ask again" : "Allow…"}</button>
            </div>
            <span class="muted small">Tyst types Ctrl+V for you through the desktop's remote input permission (asked once).</span>
          </div>
          <div class="field">
            <span class="name">Terminals</span>
            <textarea class="mono" rows="4" bind:value={terminals} onchange={saveTerminals}></textarea>
            <span class="muted small">Window classes that paste with Ctrl+Shift+V, one per line.</span>
          </div>
        {:else if view.platform === "macos"}
          <p class="muted small">Pasting needs the Accessibility permission: System Settings › Privacy &amp; Security › Accessibility.</p>
        {/if}
      {:else if tab === "meetings"}
        <h2>Meetings</h2>
        <label class="check">
          <input type="checkbox" bind:checked={c.meetings.system_audio} onchange={save} disabled={!view.system_audio_supported} />
          Transcribe system audio as “{c.labels.others}”
          {#if !view.system_audio_supported}<span class="muted">(not available on this platform yet)</span>{/if}
        </label>
        {#if view.app_filter_supported}
          <label class="check">
            <input type="checkbox" bind:checked={c.meetings.only_meeting_apps} onchange={save} disabled={!c.meetings.system_audio} />
            Only from meeting apps (leaves out music and notification sounds)
          </label>
        {/if}
        <label class="check">
          <input type="checkbox" bind:checked={c.meetings.echo_cancellation} onchange={save} disabled={!c.meetings.system_audio} />
          Remove the others' voices from your microphone (echo cancellation, for meetings on speakers)
        </label>
        <label class="check"><input type="checkbox" bind:checked={c.meetings.show_window_on_start} onchange={save} /> Show the meeting window when recording starts</label>
        <label class="check"><input type="checkbox" bind:checked={c.meetings.compact} onchange={save} /> Compact one-line window</label>
        {#if view.detect_supported}
          <label class="check"><input type="checkbox" bind:checked={c.meetings.detect} onchange={save} /> Ask to transcribe when a meeting app starts using the microphone</label>
        {/if}
        {#if (view.detect_supported && c.meetings.detect) || (view.app_filter_supported && c.meetings.only_meeting_apps)}
          <div class="field">
            <span class="name">Meeting apps</span>
            <textarea class="mono" rows="4" bind:value={meetingApps} onchange={saveMeetingApps}></textarea>
            <span class="muted small">One per line, matched against part of the app's name (a browser is probably a web meeting).{#if c.meetings.detect} Tyst only asks; it never records without your answer.{/if}{#if view.app_filter_supported && c.meetings.only_meeting_apps} “{c.labels.others}” records only these apps; add an app here if its sound is missing.{/if}</span>
          </div>
        {/if}
        <div class="field">
          <span class="name">Name prompt</span>
          <div class="row">
            <input type="number" min="5" max="300" bind:value={c.meetings.name_prompt_seconds} onchange={save} />
            <span class="muted">seconds before saving with the timestamp name</span>
          </div>
        </div>
        <p class="muted small">On speakers the microphone also hears the others. Echo cancellation takes their voices out of “{c.labels.me}” using the system audio; headphones work best.</p>
        {#if view.platform === "linux"}
          <div class="field">
            <span class="name">KDE window rule</span>
            <div class="row">
              <button onclick={() => api.installWindowRule().then(() => (status = "Window rule installed.")).catch((e) => (status = errorText(e)))}>Reinstall</button>
              <span class="muted small">Keeps the meeting window and the dictation pill on top without taking focus (System Settings › Window Rules).</span>
            </div>
          </div>
        {/if}
      {:else if tab === "models"}
        <h2>Models</h2>
        <div class="field">
          <span class="name">Models folder</span>
          <div class="row">
            <span class="mono path">{c.models_dir ?? view.default_models_dir}</span>
            <button onclick={chooseModelsFolder}>Use another folder…</button>
          </div>
        </div>
        <Models />
      {:else if tab === "vocabulary"}
        <h2>Vocabulary</h2>
        <Vocabulary />
      {:else if tab === "updates"}
        <h2>Updates</h2>
        <label class="check"><input type="checkbox" bind:checked={c.updates.check} onchange={save} /> Check for new versions on GitHub (at launch and once a day)</label>
        {#if updates}
          <div class="field">
            <span class="name">Version</span>
            <span>
              This is Tyst {updates.current}.
              {#if updates.checking}
                Checking…
              {:else if updates.available && updates.latest}
                <b>Tyst {updates.latest.version} is available.</b>
              {:else if updates.latest}
                Up to date (latest release {updates.latest.version}).
              {/if}
            </span>
            {#if updates.error}<span class="warn small">{updates.error}</span>{/if}
            <div class="row">
              <button onclick={checkNow} disabled={updates.checking}>Check now</button>
              {#if updates.latest}<button onclick={() => api.updatesOpenRelease().catch((e) => (status = errorText(e)))}>{updates.available ? "Download…" : "Release page…"}</button>{/if}
              {#if updates.last_checked}<span class="muted small">Last checked {updates.last_checked}</span>{/if}
            </div>
          </div>
          {#if updates.available && updates.latest?.notes}
            <div class="field">
              <span class="name">What's new in {updates.latest.version}</span>
              <pre class="notes">{updates.latest.notes}</pre>
            </div>
          {/if}
        {/if}
        <p class="muted small">Tyst only tells you about a new version; it never downloads or installs anything by itself.</p>
      {:else if tab === "about"}
        <h2>Tyst {view.version}</h2>
        <p>Local meeting transcription. Audio and text never leave this computer.</p>
        <p class="muted small">
          Tyst only goes online when you ask it to download the speech models, and, if update
          checks are on, to ask GitHub for the latest version. Nothing you say or write is ever sent.
        </p>
        <h3>Models</h3>
        <ul class="credits">
          <li><b>Klang Pianissimo</b> (KlangAI/pianissimo-sv), © Klang AI AB, CC BY 4.0. Tyst also runs a copy of its encoder rewritten on this computer (same weights, attention computed without padding).</li>
          <li><b>NVIDIA Parakeet TDT 0.6B v3</b>, © NVIDIA, CC BY 4.0. Only used when English is forced.</li>
          <li><b>Silero VAD</b>, © Silero Team, MIT.</li>
        </ul>
        <h3>Libraries</h3>
        <ul class="credits">
          <li><b>ONNX Runtime</b>, © Microsoft, MIT.</li>
          <li><b>WebRTC audio processing</b> (echo cancellation), © Google, BSD-3-Clause, with <b>Abseil</b>, Apache-2.0.</li>
          <li><b>Tauri</b>, MIT / Apache-2.0. <b>Svelte</b>, MIT. And many Rust crates, each under its own license.</li>
        </ul>
        <p>
          <button onclick={() => api.openNotices().catch((e) => (status = errorText(e)))}>Third-party notices…</button>
        </p>
        <p class="muted small">Tyst is licensed under MIT OR Apache-2.0.</p>
        <p class="muted small">Settings: <span class="mono">{view.config_dir}</span></p>
      {/if}
      {#if status}<p class="status">{status}</p>{/if}
    {/if}
  </main>
</div>

<style>
  .layout {
    display: flex;
    height: 100vh;
  }
  nav {
    width: 150px;
    flex: none;
    padding: 14px 8px;
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  nav button {
    text-align: left;
    border: none;
    background: transparent;
    padding: 6px 10px;
  }
  nav button.active {
    background: var(--line);
    font-weight: 600;
  }
  main {
    flex: 1;
    overflow-y: auto;
    padding: 6px 22px 22px;
  }
  h2 {
    font-size: 17px;
    margin: 12px 0 14px;
  }
  h3 {
    font-size: 13px;
    margin: 14px 0 6px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-bottom: 14px;
  }
  .name {
    font-weight: 600;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .row label {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }
  input[type="number"] {
    width: 80px;
  }
  .small {
    font-size: 12px;
  }
  .credits {
    padding-left: 18px;
  }
  .notes {
    white-space: pre-wrap;
    font: inherit;
    font-size: 12px;
    max-height: 180px;
    overflow-y: auto;
    margin: 0;
    padding: 8px;
    border: 1px solid var(--line);
    border-radius: 7px;
  }
  .credits li {
    margin-bottom: 4px;
  }
  .status,
  .warn {
    color: var(--warn);
  }
  .ok {
    color: var(--ok);
  }
  .bound {
    margin: 0;
    padding-left: 18px;
  }
  textarea {
    font: inherit;
    color: inherit;
    background: var(--field);
    border: 1px solid var(--line);
    border-radius: 7px;
    padding: 5px 8px;
    max-width: 360px;
  }
</style>

<script lang="ts">
  // Settings (SPEC 8.6): General, Meetings, Models, Vocabulary, About. Changes save at once.
  import { onMount } from "svelte";
  import { api, errorText, type ConfigView } from "../lib/api";
  import Models from "../lib/Models.svelte";
  import Vocabulary from "../lib/Vocabulary.svelte";

  const tabs = [
    ["general", "General"],
    ["meetings", "Meetings"],
    ["models", "Models"],
    ["vocabulary", "Vocabulary"],
    ["about", "About"],
  ] as const;
  type Tab = (typeof tabs)[number][0];

  let tab = $state<Tab>("general");
  let view = $state<ConfigView | null>(null);
  let status = $state<string | null>(null);

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
      {:else if tab === "meetings"}
        <h2>Meetings</h2>
        <label class="check">
          <input type="checkbox" bind:checked={c.meetings.system_audio} onchange={save} disabled={!view.system_audio_supported} />
          Transcribe system audio as “{c.labels.others}”
          {#if !view.system_audio_supported}<span class="muted">(not available on this platform yet)</span>{/if}
        </label>
        <label class="check"><input type="checkbox" bind:checked={c.meetings.show_window_on_start} onchange={save} /> Show the meeting window when recording starts</label>
        <label class="check"><input type="checkbox" bind:checked={c.meetings.compact} onchange={save} /> Compact one-line window</label>
        <div class="field">
          <span class="name">Name prompt</span>
          <div class="row">
            <input type="number" min="5" max="300" bind:value={c.meetings.name_prompt_seconds} onchange={save} />
            <span class="muted">seconds before saving with the timestamp name</span>
          </div>
        </div>
        <p class="muted small">Tip: use headphones. On speakers the microphone also hears the others and their words show up twice.</p>
        {#if view.platform === "linux"}
          <div class="field">
            <span class="name">KDE window rule</span>
            <div class="row">
              <button onclick={() => api.installWindowRule().then(() => (status = "Window rule installed.")).catch((e) => (status = errorText(e)))}>Reinstall</button>
              <span class="muted small">Keeps the meeting window on top without taking focus (System Settings › Window Rules).</span>
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
      {:else if tab === "about"}
        <h2>Tyst {view.version}</h2>
        <p>Local meeting transcription. Audio and text never leave this computer.</p>
        <h3>Models and libraries</h3>
        <ul class="credits">
          <li><b>Klang Pianissimo</b> (KlangAI/pianissimo-sv), © Klang AI AB, CC BY 4.0.</li>
          <li><b>NVIDIA Parakeet TDT 0.6B v3</b>, © NVIDIA, CC BY 4.0.</li>
          <li><b>Silero VAD</b>, MIT.</li>
          <li><b>ONNX Runtime</b>, MIT. <b>Tauri</b>, MIT / Apache-2.0. <b>Svelte</b>, MIT.</li>
        </ul>
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
  .credits li {
    margin-bottom: 4px;
  }
  .status {
    color: var(--warn);
  }
</style>

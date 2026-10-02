<script lang="ts">
  // Installed models, download (with progress) and verification. Used by onboarding and the
  // Models settings tab. Downloads only start when the user clicks.
  import { onDestroy, onMount } from "svelte";
  import { api, errorText, onModels, type ModelRow, type ModelsEvent } from "./api";

  let { compact = false, ondone }: { compact?: boolean; ondone?: () => void } = $props();

  let rows = $state<ModelRow[]>([]);
  let busy = $state(false);
  let progress = $state<{ file: string; done: number; total: number; note: string } | null>(null);
  let message = $state<string | null>(null);
  let error = $state<string | null>(null);
  let unlisten: (() => void) | undefined;

  const required = $derived(rows.filter((r) => !r.optional));
  const missingMb = $derived(required.filter((r) => r.status !== "installed").reduce((a, r) => a + r.size_mb, 0));
  const ready = $derived(required.length > 0 && required.every((r) => r.status === "installed"));

  async function refresh() {
    try {
      rows = await api.modelsStatus();
    } catch (e) {
      error = errorText(e);
    }
  }

  function handle(e: ModelsEvent) {
    if (e.type === "progress") progress = e;
    else if (e.type === "done") {
      busy = false;
      progress = null;
      message = "Models installed and verified.";
      refresh().then(() => ondone?.());
    } else {
      busy = false;
      progress = null;
      error = e.message === "download: cancelled" ? "Download paused; it resumes where it stopped." : e.message;
      refresh();
    }
  }

  async function fetch(ids: string[]) {
    error = null;
    message = null;
    busy = true;
    try {
      await api.modelsFetch(ids);
    } catch (e) {
      busy = false;
      error = errorText(e);
    }
  }

  async function verify() {
    error = null;
    message = "Checking checksums…";
    try {
      const problems = await api.modelsVerify(true);
      message = problems.length ? null : "All installed files match their checksums.";
      if (problems.length) error = problems.join("\n");
    } catch (e) {
      message = null;
      error = errorText(e);
    }
  }

  const pct = $derived(progress && progress.total > 1 ? Math.round((100 * progress.done) / progress.total) : null);

  onMount(async () => {
    unlisten = await onModels(handle);
    await refresh();
    if (ready) ondone?.();
  });
  onDestroy(() => unlisten?.());
</script>

<div class="models">
  {#if !compact}
    <table>
      <thead><tr><th>Model</th><th>Version</th><th>Size</th><th>Status</th></tr></thead>
      <tbody>
        {#each rows as r}
          <tr>
            <td>{r.id}{#if r.optional}<span class="muted"> (only for forced English)</span>{/if}</td>
            <td class="mono">{r.version.split("@")[1]}</td>
            <td>{r.size_mb.toFixed(0)} MB</td>
            <td class={`st ${r.status}`}>{r.status}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p>
      {#if ready}
        <span class="ok">✓ Models installed.</span>
      {:else}
        Pianissimo (Swedish speech recognition, Klang AI) and Silero VAD: about {missingMb.toFixed(0)} MB to download
        from Hugging Face, once.
      {/if}
    </p>
  {/if}

  {#if busy && progress}
    <div class="progress">
      <div class="bar"><div class="fill" style:width={`${pct ?? 100}%`} class:indeterminate={pct === null}></div></div>
      <span class="muted mono">{progress.file} · {progress.note}{pct !== null && progress.note === "downloading" ? ` ${pct}%` : ""}</span>
    </div>
  {/if}

  <div class="buttons">
    {#if busy}
      <button onclick={() => api.modelsCancel()}>Pause download</button>
    {:else if !ready}
      <button class="primary" onclick={() => fetch([])}>Download ({missingMb.toFixed(0)} MB)</button>
    {/if}
    {#if !compact}
      {@const parakeet = rows.find((r) => r.optional)}
      {#if parakeet && parakeet.status !== "installed" && !busy}
        <button onclick={() => fetch([parakeet.id])}>Add English model ({parakeet.size_mb.toFixed(0)} MB)</button>
      {/if}
      <button onclick={verify} disabled={busy}>Verify checksums</button>
    {/if}
  </div>

  {#if message}<p class="ok">{message}</p>{/if}
  {#if error}<pre class="error">{error}</pre>{/if}
</div>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 10px;
  }
  th,
  td {
    text-align: left;
    padding: 5px 6px;
    border-bottom: 1px solid var(--line);
  }
  th {
    font-weight: 600;
    color: var(--muted);
    font-size: 12px;
  }
  .st.installed {
    color: var(--ok);
  }
  .st.missing,
  .st.partial {
    color: var(--muted);
  }
  .st.broken {
    color: var(--warn);
  }
  .buttons {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .progress {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 8px 0 10px;
  }
  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--line);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width var(--motion);
  }
  .fill.indeterminate {
    opacity: 0.5;
  }
  .ok {
    color: var(--ok);
  }
  .error {
    color: var(--warn);
    white-space: pre-wrap;
    font: inherit;
  }
</style>

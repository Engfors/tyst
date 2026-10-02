<script lang="ts">
  // Custom vocabulary (SPEC 9.3): preferred term spellings and replacement rules, saved to
  // vocabulary.toml in the config dir; applies from the next meeting.
  import { onMount } from "svelte";
  import { api, errorText, type Vocabulary } from "./api";

  let vocab = $state<Vocabulary>({ terms: [], replacements: [] });
  let newTerm = $state("");
  let newFrom = $state("");
  let newTo = $state("");
  let status = $state<string | null>(null);

  async function persist() {
    try {
      await api.vocabularySet($state.snapshot(vocab));
      status = "Saved. Applies from the next meeting.";
    } catch (e) {
      status = errorText(e);
    }
  }

  function addTerm(e: Event) {
    e.preventDefault();
    const t = newTerm.trim();
    if (!t || vocab.terms.includes(t)) return;
    vocab.terms = [...vocab.terms, t].sort((a, b) => a.localeCompare(b));
    newTerm = "";
    persist();
  }

  function addRule(e: Event) {
    e.preventDefault();
    const from = newFrom.trim();
    const to = newTo.trim();
    if (!from || !to) return;
    vocab.replacements = [...vocab.replacements.filter((r) => r.from.toLowerCase() !== from.toLowerCase()), { from, to }];
    newFrom = "";
    newTo = "";
    persist();
  }

  async function importFile() {
    try {
      const v = await api.vocabularyImport();
      if (v) {
        vocab = v;
        status = "Imported.";
      }
    } catch (e) {
      status = errorText(e);
    }
  }

  async function exportFile() {
    try {
      const p = await api.vocabularyExport();
      if (p) status = `Exported to ${p}`;
    } catch (e) {
      status = errorText(e);
    }
  }

  onMount(async () => {
    vocab = await api.vocabularyGet();
  });
</script>

<div class="vocab">
  <section>
    <h3>Terms</h3>
    <p class="muted">Preferred spellings. Exact matches are written this way, whatever the casing.</p>
    <form class="add" onsubmit={addTerm}>
      <input type="text" bind:value={newTerm} placeholder="e.g. HashiCorp" />
      <button type="submit">Add</button>
    </form>
    <ul class="chips">
      {#each vocab.terms as t}
        <li>
          {t}
          <button
            title="Remove"
            onclick={() => {
              vocab.terms = vocab.terms.filter((x) => x !== t);
              persist();
            }}>×</button
          >
        </li>
      {/each}
    </ul>
  </section>

  <section>
    <h3>Replacements</h3>
    <p class="muted">Fix systematic misrecognitions. Whole words, any casing.</p>
    <form class="add" onsubmit={addRule}>
      <input type="text" bind:value={newFrom} placeholder="terra form" />
      <span class="muted">→</span>
      <input type="text" bind:value={newTo} placeholder="Terraform" />
      <button type="submit">Add</button>
    </form>
    <table>
      <tbody>
        {#each vocab.replacements as r}
          <tr>
            <td>{r.from}</td>
            <td class="muted">→</td>
            <td>{r.to}</td>
            <td class="rm">
              <button
                title="Remove"
                onclick={() => {
                  vocab.replacements = vocab.replacements.filter((x) => x !== r);
                  persist();
                }}>×</button
              >
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </section>

  <div class="buttons">
    <button onclick={importFile}>Import…</button>
    <button onclick={exportFile}>Export…</button>
    {#if status}<span class="muted">{status}</span>{/if}
  </div>
</div>

<style>
  h3 {
    margin: 0 0 2px;
    font-size: 13px;
  }
  section {
    margin-bottom: 16px;
  }
  p {
    margin: 0 0 8px;
  }
  .add {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-bottom: 8px;
  }
  .add input {
    flex: 1;
    min-width: 0;
  }
  .chips {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chips li {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 9px;
    border: 1px solid var(--line);
    border-radius: 12px;
  }
  .chips button,
  .rm button {
    border: none;
    background: transparent;
    padding: 0 5px;
    color: var(--muted);
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 3px 6px;
    border-bottom: 1px solid var(--line);
  }
  .rm {
    width: 30px;
    text-align: right;
  }
  .buttons {
    display: flex;
    gap: 8px;
    align-items: center;
  }
</style>

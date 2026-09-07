<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { snapshot } from "./lib/stores";
  import { mergeTitleEntries, type TitleEntry } from "./lib/titleEntries";

  interface Settings {
    pollIntervalSecs: number;
    titleEntries: TitleEntry[];
    thresholds: number[];
    notificationsEnabled: boolean;
    launchAtLogin: boolean;
  }

  let settings = $state<Settings | null>(null);
  let saved = $state(false);
  let loadError = $state<string | null>(null);
  let saveError = $state<string | null>(null);

  invoke<Settings>("get_settings")
    .then((s) => (settings = s))
    .catch((e) => (loadError = String(e)));

  // The settings window's webview is alive from app launch (it just starts
  // hidden), so it is usually already subscribed and caught up by the time
  // someone opens it. The one gap is a window opened before any snapshot has
  // ever landed — ask for one rather than leaving the menu-bar list empty
  // until the next scheduled poll.
  $effect(() => {
    if (!$snapshot) {
      invoke("refresh_now").catch(() => {});
    }
  });

  // One row per quota the app currently knows about, in addition to the ones
  // already in titleEntries — otherwise a quota that was never added to the
  // menu bar before (or is new) can never be ticked on. `mergeTitleEntries`
  // is a union, never a replacement (R43): an empty or absent snapshot (no
  // snapshot yet, signed out, a transient blip) leaves existing entries
  // untouched rather than wiping them, and an entry whose quota is missing
  // from this particular snapshot survives too.
  $effect(() => {
    if (!settings) return;
    settings.titleEntries = mergeTitleEntries(
      settings.titleEntries,
      $snapshot?.snapshot.quotas ?? [],
    );
  });

  function labelFor(quotaId: string): string {
    return $snapshot?.snapshot.quotas.find((q) => q.id === quotaId)?.label ?? quotaId;
  }

  async function persist() {
    if (!settings) return;
    saveError = null;
    try {
      await invoke("set_settings", { settings });
      saved = true;
      setTimeout(() => (saved = false), 1500);
    } catch (e) {
      saveError = String(e);
    }
  }

  // `sanitized()` on the Rust side already sorts, deduplicates and bounds
  // these to 1..=100 on load and on save, and empties the list whenever
  // notifications are disabled — this only has to not fight that. It does
  // not re-validate as the user types: a value that is momentarily out of
  // range or duplicated while someone is mid-edit is exactly the kind of
  // "list looks empty or wrong for a moment" state that must survive
  // untouched rather than being "corrected" out from under them.
  function addThreshold() {
    if (!settings) return;
    const highest = settings.thresholds.length ? Math.max(...settings.thresholds) : 40;
    settings.thresholds = [...settings.thresholds, Math.min(100, highest + 10)];
  }

  function removeThreshold(index: number) {
    if (!settings) return;
    settings.thresholds = settings.thresholds.filter((_, i) => i !== index);
  }
</script>

{#if loadError}
  <main>
    <p class="error">Could not load settings: {loadError}</p>
  </main>
{:else if settings}
  <main>
    <label>
      Refresh every
      <input type="number" min="30" step="10" bind:value={settings.pollIntervalSecs} />
      seconds
    </label>

    <label class="check">
      <input type="checkbox" bind:checked={settings.notificationsEnabled} />
      Notify me at these usage levels
    </label>

    <div class="thresholds" class:disabled={!settings.notificationsEnabled}>
      {#each settings.thresholds as _, i}
        <span class="threshold">
          <input
            type="number"
            min="1"
            max="100"
            disabled={!settings.notificationsEnabled}
            bind:value={settings.thresholds[i]}
          />
          <span class="pct">%</span>
          <button
            type="button"
            class="remove"
            disabled={!settings.notificationsEnabled}
            onclick={() => removeThreshold(i)}
            aria-label="Remove threshold"
          >&times;</button>
        </span>
      {/each}
      {#if settings.thresholds.length === 0}
        <span class="waiting">No thresholds set</span>
      {/if}
      <button
        type="button"
        class="add"
        disabled={!settings.notificationsEnabled}
        onclick={addThreshold}
      >+ Add</button>
    </div>

    <label class="check">
      <input type="checkbox" bind:checked={settings.launchAtLogin} />
      Launch at login
    </label>

    <fieldset>
      <legend>Menu bar</legend>
      {#if settings.titleEntries.length === 0}
        <p class="waiting">Waiting for usage data…</p>
      {/if}
      {#each settings.titleEntries as entry (entry.quotaId)}
        <div class="entry">
          <span>{labelFor(entry.quotaId)}</span>
          <label><input type="checkbox" bind:checked={entry.showPercent} /> %</label>
          <label><input type="checkbox" bind:checked={entry.showCountdown} /> countdown</label>
        </div>
      {/each}
    </fieldset>

    <footer>
      <button onclick={persist}>Save</button>
      {#if saved}<span class="ok">Saved</span>{/if}
      {#if saveError}<span class="error">Could not save: {saveError}</span>{/if}
    </footer>
  </main>
{/if}

<style>
  main { padding: 16px; display: flex; flex-direction: column; gap: 10px; }
  label { display: flex; align-items: center; gap: 8px; }
  label.check { gap: 8px; }
  input[type="number"] { width: 72px; }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 12px;
    max-height: 150px;
    overflow-y: auto;
  }
  .entry { display: flex; gap: 12px; align-items: center; padding: 4px 0; }
  .entry span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .waiting { color: var(--fg-muted); font-size: 12px; margin: 4px 0; }
  .thresholds {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: -2px 0 2px 24px;
  }
  .thresholds.disabled { opacity: 0.5; }
  .threshold {
    display: flex;
    align-items: center;
    gap: 2px;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 2px 4px 2px 8px;
  }
  .threshold input[type="number"] { width: 44px; }
  .threshold .pct { color: var(--fg-muted); font-size: 12px; }
  .threshold .remove,
  .add {
    background: none;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    border-radius: 5px;
    padding: 2px 6px;
    font: inherit;
  }
  .threshold .remove:hover,
  .add:hover:not(:disabled) { color: var(--fg); background: var(--hover); }
  .add { border: 1px dashed var(--border); }
  .threshold .remove:disabled,
  .add:disabled { cursor: default; }
  footer { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .ok { color: var(--normal); font-size: 12px; }
  .error { color: var(--critical); font-size: 12px; }
</style>

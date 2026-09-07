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
    analyticsEnabled: boolean;
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
      Notify me at {settings.thresholds.join(", ")}%
    </label>

    <label class="check">
      <input type="checkbox" bind:checked={settings.launchAtLogin} />
      Launch at login
    </label>

    <label class="check">
      <input type="checkbox" bind:checked={settings.analyticsEnabled} />
      Show local token &amp; cost analytics
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
    border: 1px solid var(--hairline);
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
  footer { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .ok { color: var(--normal); font-size: 12px; }
  .error { color: var(--critical); font-size: 12px; }
</style>

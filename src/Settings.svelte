<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  interface TitleEntry {
    quotaId: string;
    showPercent: boolean;
    showCountdown: boolean;
  }
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

  invoke<Settings>("get_settings").then((s) => (settings = s));

  async function persist() {
    if (!settings) return;
    await invoke("set_settings", { settings });
    saved = true;
    setTimeout(() => (saved = false), 1500);
  }
</script>

{#if settings}
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
      {#each settings.titleEntries as entry (entry.quotaId)}
        <div class="entry">
          <span>{entry.quotaId}</span>
          <label><input type="checkbox" bind:checked={entry.showPercent} /> %</label>
          <label><input type="checkbox" bind:checked={entry.showCountdown} /> countdown</label>
        </div>
      {/each}
    </fieldset>

    <footer>
      <button onclick={persist}>Save</button>
      {#if saved}<span class="ok">Saved</span>{/if}
    </footer>
  </main>
{/if}

<style>
  main { padding: 18px; display: flex; flex-direction: column; gap: 14px; }
  label { display: flex; align-items: center; gap: 8px; }
  label.check { gap: 8px; }
  input[type="number"] { width: 72px; }
  fieldset { border: 1px solid var(--hairline); border-radius: 8px; padding: 10px 12px; }
  .entry { display: flex; gap: 12px; align-items: center; padding: 4px 0; }
  .entry span { flex: 1; font-variant-numeric: tabular-nums; }
  footer { display: flex; align-items: center; gap: 10px; }
  .ok { color: var(--normal); font-size: 12px; }
</style>

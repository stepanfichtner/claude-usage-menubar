<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { now, snapshot } from "./lib/stores";
  import {
    mergeTitleEntries,
    retireAbsentEntries,
    type Absences,
    type TitleEntry,
  } from "./lib/titleEntries";
  import { renderTitle } from "./lib/titlePreview";

  interface Settings {
    pollIntervalSecs: number;
    titleEntries: TitleEntry[];
    thresholds: number[];
    notificationsEnabled: boolean;
    launchAtLogin: boolean;
  }

  // The only values worth offering: see `MIN_POLL_INTERVAL_SECS`'s doc
  // comment in src-tauri/src/settings.rs for why 60s is the floor, and why
  // there is no preset below it.
  const POLL_INTERVAL_PRESETS = [
    { value: 60, label: "Every minute" },
    { value: 120, label: "Every 2 minutes" },
    { value: 180, label: "Every 3 minutes" },
    { value: 300, label: "Every 5 minutes" },
    { value: 600, label: "Every 10 minutes" },
  ];

  function isKnownPollInterval(value: number): boolean {
    return POLL_INTERVAL_PRESETS.some((preset) => preset.value === value);
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

  // Consecutive-absence tallies for `retireAbsentEntries`, deliberately a
  // plain `let` and not `$state`: nothing renders it, and writing reactive
  // state from the effect that reads it would re-trigger the effect. It
  // starts empty every time this window's webview is created, which only
  // ever makes retirement slower.
  let absences: Absences = {};

  // One row per quota the app currently knows about, in addition to the ones
  // already in titleEntries — otherwise a quota that was never added to the
  // menu bar before (or is new) can never be ticked on. `mergeTitleEntries`
  // is a union, never a replacement (R43): an empty or absent snapshot (no
  // snapshot yet, signed out, a transient blip) leaves existing entries
  // untouched rather than wiping them, and an entry whose quota is missing
  // from this particular snapshot survives too.
  //
  // `retireAbsentEntries` is the bounded exception: an entry still missing
  // after `RETIREMENT_MISSES` consecutive live, non-empty, non-signed-out
  // snapshots is dropped, so a quota retired by a plan change stops showing
  // a raw id above two checkboxes that cannot do anything. It keeps R43 —
  // every untrustworthy snapshot returns everything untouched — and it only
  // changes the in-memory list; nothing reaches the store until Save.
  $effect(() => {
    if (!settings) return;
    const event = $snapshot;
    const merged = mergeTitleEntries(settings.titleEntries, event?.snapshot.quotas ?? []);
    const retirement = retireAbsentEntries(
      merged,
      event && { ...event.snapshot, signedOut: event.signedOut },
      absences,
    );
    absences = retirement.absences;
    settings.titleEntries = retirement.entries;
  });

  function labelFor(quotaId: string): string {
    return $snapshot?.snapshot.quotas.find((q) => q.id === quotaId)?.label ?? quotaId;
  }

  // Built entirely from data this window already holds — the subscribed
  // snapshot plus the checkboxes below — so it costs no extra command or
  // round trip. `renderTitle` (src/lib/titlePreview.ts) mirrors
  // `tray::render_title` on the Rust side and carries its own tests.
  let previewTitle = $derived(
    settings ? renderTitle($snapshot?.snapshot.quotas ?? [], settings.titleEntries, $now) : "",
  );

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
    <div class="columns">
      <section class="panel">
        <h2>General</h2>

        <div class="group">
          <label>
            Refresh every
            <select bind:value={settings.pollIntervalSecs}>
              {#each POLL_INTERVAL_PRESETS as preset (preset.value)}
                <option value={preset.value}>{preset.label}</option>
              {/each}
              {#if !isKnownPollInterval(settings.pollIntervalSecs)}
                <option value={settings.pollIntervalSecs}>
                  {settings.pollIntervalSecs} seconds (current)
                </option>
              {/if}
            </select>
          </label>
        </div>

        <div class="group">
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
        </div>

        <div class="group">
          <label class="check">
            <input type="checkbox" bind:checked={settings.launchAtLogin} />
            Launch at login
          </label>
        </div>
      </section>

      <section class="panel">
        <h2>Menu bar</h2>
        <p class="hint">
          Choose what each usage limit shows next to the icon in your menu bar.
        </p>

        {#if settings.titleEntries.length === 0}
          <p class="waiting">Waiting for usage data…</p>
        {:else}
          <div class="quota-list">
            {#each settings.titleEntries as entry (entry.quotaId)}
              <div class="quota-group">
                <p class="quota-name">{labelFor(entry.quotaId)}</p>
                <label class="check">
                  <input type="checkbox" bind:checked={entry.showPercent} />
                  Percentage used
                </label>
                <label class="check">
                  <input type="checkbox" bind:checked={entry.showCountdown} />
                  Time until reset
                </label>
              </div>
            {/each}
          </div>

          <div class="preview">
            <span class="preview-label">Menu bar preview</span>
            <span class="preview-value">{previewTitle || "(nothing shown)"}</span>
          </div>
        {/if}
      </section>
    </div>

    <footer>
      <button class="primary" onclick={persist}>Save</button>
      {#if saved}<span class="ok">Saved</span>{/if}
      {#if saveError}<span class="error">Could not save: {saveError}</span>{/if}
    </footer>
  </main>
{/if}

<style>
  main {
    box-sizing: border-box;
    height: 100vh;
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow-y: auto;
  }
  .columns {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 20px;
    align-items: start;
  }
  .panel {
    box-sizing: border-box;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .panel h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .hint {
    margin: -8px 0 0;
    color: var(--fg-muted);
    font-size: 12px;
    line-height: 1.5;
  }
  .group + .group {
    border-top: 1px solid var(--border);
    padding-top: 14px;
  }
  label { display: flex; align-items: center; gap: 8px; }
  label.check { gap: 8px; }
  select {
    font: inherit;
    color: inherit;
    background: var(--track);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 6px;
  }
  input[type="number"] { width: 60px; }

  /* One threshold per row. They wrapped into an uneven grid before, which
     made "+ Add" look like a fourth threshold on the second line and left the
     reading order ambiguous — a stacked list is scanned top to bottom and the
     add button is unmistakably the end of it. */
  .thresholds {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    margin: 8px 0 0 24px;
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

  .quota-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    /* Fits the two default quotas with a little headroom to spare — the
       extra bottom padding means that headroom is unused blank space, not a
       border sitting flush against the cap. A third or fourth (the API can
       return up to four: session plus three weekly variants) scrolls within
       this list rather than resizing the window around an uncommon case; no
       fade on the edge, since a partially visible header on a scrolled-past
       group is the one cue this list has that there's more below it. */
    max-height: 200px;
    overflow-y: auto;
    padding-right: 2px;
    padding-bottom: 8px;
  }
  .quota-group {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
  }
  .quota-name {
    margin: 0 0 6px;
    font-weight: 600;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .quota-group .check { padding: 2px 0; font-size: 12px; }

  .preview {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    border-top: 1px solid var(--border);
    padding-top: 12px;
  }
  .preview-label { color: var(--fg-muted); font-size: 12px; }
  .preview-value {
    font-variant-numeric: tabular-nums;
    background: var(--track);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 13px;
    /* `renderTitle` joins entries with two literal spaces (mirroring
       `tray.rs`'s own join) specifically to keep separate quotas apart from
       each other and from the " · " within one quota's segment. Plain
       `white-space` would collapse that pair to one space, making the two
       gaps indistinguishable; `pre-wrap` preserves it while still wrapping
       at whitespace when the line is too long for the box. */
    white-space: pre-wrap;
    overflow-wrap: break-word;
  }

  .waiting { color: var(--fg-muted); font-size: 12px; margin: 4px 0; }

  footer { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
  button.primary {
    background: var(--fg);
    color: var(--bg);
    border: none;
    border-radius: 8px;
    padding: 12px 28px;
    font: inherit;
    font-weight: 600;
    font-size: 14px;
    cursor: pointer;
  }
  button.primary:hover { opacity: 0.88; }
  button.primary:active { opacity: 0.76; }
  .ok { color: var(--normal); font-size: 12px; font-weight: 600; }
  .error { color: var(--critical); font-size: 12px; }
</style>

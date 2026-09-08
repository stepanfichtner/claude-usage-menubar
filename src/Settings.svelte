<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { now, snapshot } from "./lib/stores";
  import {
    NO_RETIREMENT_YET,
    reconcileTitleEntries,
    type RetirementState,
    type TitleEntry,
  } from "./lib/titleEntries";
  import { nextThreshold } from "./lib/thresholds";
  import { renderTitle } from "./lib/titlePreview";

  interface Settings {
    pollIntervalSecs: number;
    titleEntries: TitleEntry[];
    thresholds: number[];
    notificationsEnabled: boolean;
    launchAtLogin: boolean;
    analyticsEnabled: boolean;
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

  // Retirement bookkeeping, deliberately a plain `let` and not `$state`:
  // nothing renders it, and writing reactive state from the effect that reads
  // it would re-trigger the effect. It starts empty every time this window's
  // webview is created, which only ever makes retirement slower.
  let retirement: RetirementState = NO_RETIREMENT_YET;

  // One row per quota the app currently knows about, in addition to the ones
  // already in titleEntries — otherwise a quota that was never added to the
  // menu bar before (or is new) can never be ticked on. `mergeTitleEntries`
  // is a union, never a replacement (R43): an empty or absent snapshot (no
  // snapshot yet, signed out, a transient blip) leaves existing entries
  // untouched rather than wiping them, and an entry whose quota is missing
  // from this particular snapshot survives too.
  //
  // Retirement is the bounded exception: an entry still missing after
  // `RETIREMENT_MISSES` consecutive live, non-empty, non-signed-out snapshots
  // is dropped, so a quota retired by a plan change stops showing a raw id
  // above two checkboxes that cannot do anything. It keeps R43 — every
  // untrustworthy snapshot returns everything untouched — and it only changes
  // the in-memory list; nothing reaches the store until Save.
  //
  // This effect can run more than once per snapshot: it reads
  // `settings.titleEntries` and writes it back, and `mergeTitleEntries`
  // returns a fresh array whenever it appends, so an append re-triggers the
  // read. `reconcileTitleEntries` counts each snapshot once by `fetchedAt`
  // rather than once per call, so the tally does not depend on how often this
  // runs; the whole rule lives there and is tested there.
  $effect(() => {
    if (!settings) return;
    const event = $snapshot;
    const next = reconcileTitleEntries(
      settings.titleEntries,
      event && { ...event.snapshot, signedOut: event.signedOut },
      retirement,
    );
    retirement = next.state;
    settings.titleEntries = next.entries;
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
  // `nextThreshold` (src/lib/thresholds.ts) answers both questions the
  // button has: what to append, and whether there is anything left to
  // append at all. It used to return 100 a second time once 100 was set,
  // and the UI showed the impossible duplicate row until it reloaded.
  let proposedThreshold = $derived(settings ? nextThreshold(settings.thresholds) : null);

  function addThreshold() {
    if (!settings || proposedThreshold === null) return;
    settings.thresholds = [...settings.thresholds, proposedThreshold];
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
            <span class="add-row">
              <button
                type="button"
                class="add"
                disabled={!settings.notificationsEnabled || proposedThreshold === null}
                onclick={addThreshold}
              >+ Add</button>
              <!-- Says why the button is dead, rather than leaving a click do
                   nothing. Only while notifications are on: with them off the
                   whole block is dimmed and disabled for a different reason,
                   and two explanations at once explains neither. -->
              {#if settings.notificationsEnabled && proposedThreshold === null}
                <span class="ceiling">100% is already the highest</span>
              {/if}
            </span>
          </div>
        </div>

        <!-- Two plain toggles, one rule above them rather than one between
             them: a divider separating two single-line checkboxes reads as a
             section break where there is no section. -->
        <div class="group">
          <label class="check">
            <input type="checkbox" bind:checked={settings.launchAtLogin} />
            Launch at login
          </label>

          <!-- Removed for 0.1.0, which had no backend for it, and back now
               that it does something. `analyticsEnabled` stayed in the stored
               settings throughout, so a store written by 0.1.0 needs no
               migration. -->
          <label class="check">
            <input type="checkbox" bind:checked={settings.analyticsEnabled} />
            Show local token &amp; cost analytics
          </label>
          <p class="hint indent">
            Adds a Usage tab estimating tokens and cost from Claude Code's
            local transcripts. Nothing leaves this machine.
          </p>
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
  /* The window scrolls its content, never its Save button. `main` itself does
     not scroll — `.columns` does — so the footer stays on screen however many
     quotas there are. It used to sit below the columns inside one scrolling
     box, which put Save under the fold exactly when someone had enough quotas
     to want it. */
  main {
    box-sizing: border-box;
    height: 100vh;
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow: hidden;
  }
  .columns {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 20px;
    align-items: start;
    /* `min-height: 0` is what actually lets this shrink inside the flex
       column; without it a grid child refuses to go below its content and
       scrolls the whole window instead. */
    flex: 1;
    min-height: 0;
    overflow-y: auto;
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
  /* Aligned under the checkbox's label rather than its box, so it reads as
     that setting's explanation and not as a new paragraph. */
  .hint.indent { margin: 2px 0 0 24px; }
  /* Separates the two toggles just enough that the hint below reads as
     belonging to the second one rather than to the pair. */
  .group label.check + label.check { margin-top: 8px; }
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
  /* The button and its reason share a line: the thresholds column is a
     narrow left-aligned stack, and a reason on its own row reads as a
     status message about the whole list rather than about this button. */
  .add-row { display: flex; align-items: center; gap: 8px; }
  .ceiling { color: var(--fg-muted); font-size: 12px; }

  .quota-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    /* No cap and no scroll of its own. This list used to stop at 200px and
       scroll inside a window that also scrolled, so a third quota put the
       reader in a box inside a box — and the outer one had already pushed
       Save under the fold. One scroll region for the whole window, and a
       window the user can drag taller, beats two nested ones sized for a
       guess about how many quotas the API returns. */
    padding-right: 2px;
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

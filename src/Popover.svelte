<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import SettingsIcon from "@lucide/svelte/icons/settings";
  import Callout from "./lib/Callout.svelte";
  import Ring from "./lib/Ring.svelte";
  import QuotaCard from "./lib/QuotaCard.svelte";
  import { formatCompact } from "./lib/countdown";
  import { ageIsStale, formatAge, secondsSince } from "./lib/freshness";
  import { PANEL_WIDTH, nextPanelHeight, splitQuotas } from "./lib/panel";
  import { now, snapshot } from "./lib/stores";

  // Everything here that can be decided without a webview lives in
  // `./lib/panel` and `./lib/freshness`, where the vitest suite reaches it:
  // the window's resize arithmetic, the quota split, and the footer's age and
  // staleness. What is left in this file needs a real Tauri window or a real
  // render — the `app_version` round trip, `getCurrentWindow()`, the refresh
  // acknowledgement timer, the `hasLiveSnapshot` latch, `fetchedAtLabel`'s
  // locale formatting, and the `{#if}` chains in the markup below. None of
  // that is covered by tests: this repo has no component-rendering harness,
  // and a jsdom render would not be the environment this panel actually runs
  // in anyway.

  // Read once at startup from Cargo.toml's version via the `app_version`
  // command. Empty until it resolves, and the footer simply omits it in that
  // window rather than rendering a placeholder that flashes.
  let version = $state("");
  invoke<string>("app_version")
    .then((v) => (version = v))
    .catch(() => {});

  // Outside a real Tauri webview — this component mounted in a plain browser
  // for headless verification, say — `getCurrentWindow()` throws
  // synchronously (it reads `window.__TAURI_INTERNALS__`, which only exists
  // inside an actual Tauri window). There's no OS window to resize in that
  // case, so treat it as "don't try to resize," not a crash.
  let tauriWindow: ReturnType<typeof getCurrentWindow> | null = null;
  try {
    tauriWindow = getCurrentWindow();
  } catch {
    tauriWindow = null;
  }

  let mainEl: HTMLElement | undefined = $state();

  // Acknowledges the click, nothing more. A throttled or rate-limited
  // refresh emits no snapshot event at all (poller.rs's `Decision::Wait`
  // path), so waiting for "the next snapshot" to clear this would leave the
  // icon spinning forever on exactly the refreshes most worth acknowledging.
  // Stopping on a fixed timer instead means the acknowledgement never
  // depends on something that may never happen.
  let refreshing = $state(false);

  async function refresh() {
    refreshing = true;
    try {
      await invoke("refresh_now");
    } finally {
      setTimeout(() => (refreshing = false), 1000);
    }
  }

  // The last height actually requested from the OS window; `nextPanelHeight`
  // answers `null` when it hasn't changed, and explains why that matters. `0`
  // is the "nothing requested yet" sentinel and is not a height any real
  // snapshot produces, so the first resize is never skipped.
  let lastHeight = 0;

  // `stale` means "no live fetch has succeeded since launch" — true only for
  // the cached snapshot served at cold start, before the first live poll
  // lands. Once a live (non-stale) snapshot has arrived this session, it
  // cannot mean that any more, so track it independently rather than trusting
  // any single event's flag.
  let hasLiveSnapshot = $state(false);
  $effect(() => {
    if ($snapshot && !$snapshot.snapshot.stale) hasLiveSnapshot = true;
  });

  const ageSeconds = $derived(secondsSince($snapshot?.snapshot.fetchedAt, $now));
  const age = $derived(formatAge(ageSeconds));

  // The exact timestamp, on hover. The relative age answers "is this current?"
  // at a glance; this answers "current as of when?" without spending a line of
  // a 320px panel on it.
  const fetchedAtLabel = $derived.by(() => {
    const fetchedAt = $snapshot?.snapshot.fetchedAt;
    if (!fetchedAt) return "";
    return new Date(fetchedAt).toLocaleString();
  });

  // The genuine cold-start case only: cached data, and no live snapshot has
  // arrived yet. A poll that merely failed (rate limited, offline) no longer
  // touches `stale` at all, so it never lands here — the footer just keeps
  // counting up the age of the last good snapshot instead.
  const showingCachedData = $derived(!hasLiveSnapshot && ($snapshot?.snapshot.stale ?? false));
  const footerIsWarning = $derived(ageIsStale(ageSeconds));

  const layout = $derived(splitQuotas($snapshot?.snapshot.quotas ?? []));
  const session = $derived(layout.session);
  const rings = $derived(layout.rings);

  // The quota count comes from the server (`limits[]`), so a fixed window
  // height either wastes space (one quota) or clips the session card and
  // footer below the fold (several) — a fixed-size window can't be right for
  // content whose size we don't control. Resize to fit instead, whenever the
  // content changes.
  //
  // Measuring `mainEl.scrollHeight` rather than `document.body.scrollHeight`
  // matters here — and specifically must not be measured on an element with
  // a *fixed* `height` (a first attempt at this used `height: 100vh` on
  // `main` and measured from there: `scrollHeight` is `max(own set height,
  // content height)`, so it silently floored every measurement at the
  // window's current height and could shrink to fit but never grow, nor
  // shrink below whatever height the window already happened to be). `main`
  // below uses `max-height: 100vh` instead — a ceiling, not a fixed size —
  // so it sizes to its content in both directions, and `scrollHeight`
  // reports that true content height, only bottoming out at `max-height`
  // once content genuinely exceeds it (see the stylesheet below).
  $effect(() => {
    void $snapshot; // re-measure whenever the content changes
    if (!tauriWindow) return;
    requestAnimationFrame(() => {
      if (!mainEl) return;
      const height = nextPanelHeight(mainEl.scrollHeight, lastHeight);
      if (height === null) return;
      lastHeight = height;
      tauriWindow!.setSize(new LogicalSize(PANEL_WIDTH, height)).catch(() => {});
    });
  });
</script>

<main bind:this={mainEl}>
  <header>
    <div class="who">
      <div class="name">{$snapshot?.profile?.displayName ?? "Claude usage"}</div>
      {#if $snapshot?.profile?.planLabel}
        <div class="plan">{$snapshot.profile.planLabel}</div>
      {/if}
    </div>
    <button class="icon-button" class:busy={refreshing} title="Refresh now" onclick={refresh}>
      <RefreshCw size={16} />
    </button>
    <button title="Settings" onclick={() => invoke("open_settings")}>
      <SettingsIcon size={16} />
    </button>
  </header>

  {#if $snapshot?.signedOut}
    <p class="empty">Sign in to Claude Code to see your usage.</p>
  {:else if !$snapshot}
    <p class="empty">Loading…</p>
  {:else}
    {#if rings.length > 0}
      <div class="rings">
        {#each rings as quota (quota.id)}
          <div class="ring-card" class:active={quota.isActive}>
            <Ring percent={quota.percent} severity={quota.severity} />
            <span class="ring-label">{quota.label}</span>
            <span class="ring-meta">
              {quota.resetsAt ? `${formatCompact(quota.resetsAt, $now)} left` : "—"}
            </span>
          </div>
        {/each}
      </div>
    {/if}

    {#if session}
      <QuotaCard quota={session} now={$now} />
    {/if}

    {#if showingCachedData}
      <Callout tone="warning">Showing cached data</Callout>
    {/if}
    <footer class:stale={footerIsWarning}>
      <span class="freshness" title={fetchedAtLabel}>
        <span class="dot"></span>{age}
      </span>
      {#if version}<span class="version">v{version}</span>{/if}
    </footer>
  {/if}
</main>

<style>
  main {
    background: var(--bg);
    backdrop-filter: blur(30px);
    border-radius: 12px;
    padding: 6px;
    /* Not `height: 100vh` — that would force `main.scrollHeight` (what the
       resize effect above measures) to bottom out at the *current* window
       height even when the content is shorter, since `scrollHeight` is
       max(own set height, content height). Left to size naturally, it
       reports the true content height in both directions; `max-height` is
       purely a fallback so content can never render taller than the OS
       window if a resize is ever denied or hasn't landed yet. */
    max-height: 100vh;
    overflow-y: auto;
  }
  header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px 10px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 8px;
  }
  .who { flex: 1; min-width: 0; }
  .name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .plan { font-size: 11px; color: var(--fg-muted); }
  button {
    display: flex;
    background: none;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 5px;
    border-radius: 5px;
  }
  button:hover { color: var(--fg); background: var(--hover); }
  .icon-button.busy :global(svg) { animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) {
    .icon-button.busy :global(svg) { animation: none; opacity: 0.5; }
  }
  .empty { padding: 24px 14px; text-align: center; color: var(--fg-muted); }

  .rings {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
    padding: 0 8px 8px;
  }
  .ring-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 14px 8px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    /* Without this, a grid item won't shrink below its content's intrinsic
       width, so a long label's `nowrap` blows out the card — and the whole
       2-column grid — past the panel's 320px. */
    min-width: 0;
  }
  .ring-card.active { border-color: var(--fg-muted); }
  .ring-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--fg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .ring-meta { font-size: 11px; color: var(--fg-muted); font-variant-numeric: tabular-nums; }

  /* A status bar, not a caption: the rule separates it from the quotas above,
     and the two ends answer different questions — how fresh the numbers are,
     and which build is showing them. */
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 4px;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    font-size: 11px;
    color: var(--fg-muted);
  }
  .freshness { display: inline-flex; align-items: center; gap: 6px; }
  /* Inherits the footer's colour, so it turns amber with the text when the
     snapshot goes stale rather than needing its own rule. */
  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: currentColor;
    opacity: 0.5;
    flex: none;
  }
  footer.stale { color: var(--warning); }
  footer.stale .dot { opacity: 1; }
  /* Pinned to the muted ink rather than inheriting: when the snapshot goes
     stale the footer turns amber, and that warning is about the data's age.
     Letting the version turn amber with it would claim something is wrong
     with the build. */
  .version {
    color: var(--fg-muted);
    opacity: 0.8;
    font-variant-numeric: tabular-nums;
  }
</style>

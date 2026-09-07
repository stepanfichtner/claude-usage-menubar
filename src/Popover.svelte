<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import SettingsIcon from "@lucide/svelte/icons/settings";
  import Callout from "./lib/Callout.svelte";
  import Ring from "./lib/Ring.svelte";
  import QuotaCard from "./lib/QuotaCard.svelte";
  import { formatCompact } from "./lib/countdown";
  import { now, snapshot } from "./lib/stores";

  const STALE_WARNING_SECS = 5 * 60;

  // The window's fixed width, matching `tauri.conf.json`'s popover window —
  // only the height ever changes. Bounds are a sane floor/ceiling so a
  // pathological snapshot (zero quotas, or fifty of them) can't produce an
  // unusably short or absurdly tall window; ordinary quota counts (1-6ish)
  // land well inside this range, and `overflow-y: auto` on `main` below is
  // the fallback if a real one ever doesn't.
  const PANEL_WIDTH = 320;
  const MIN_HEIGHT = 140;
  const MAX_HEIGHT = 720;

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

  // `stale` means "no live fetch has succeeded since launch" — true only for
  // the cached snapshot served at cold start, before the first live poll
  // lands. Once a live (non-stale) snapshot has arrived this session, it
  // cannot mean that any more, so track it independently rather than trusting
  // any single event's flag.
  let hasLiveSnapshot = $state(false);
  $effect(() => {
    if ($snapshot && !$snapshot.snapshot.stale) hasLiveSnapshot = true;
  });

  const ageSeconds = $derived.by(() => {
    const fetchedAt = $snapshot?.snapshot.fetchedAt;
    if (!fetchedAt) return 0;
    return Math.max(0, Math.floor(($now.getTime() - new Date(fetchedAt).getTime()) / 1000));
  });

  // Elapsed time, computed directly. Reusing formatLong here would be wrong:
  // it answers "how long until this timestamp", and for one already in the past
  // it returns "now" — rendering "updated now ago".
  const age = $derived.by(() => {
    const seconds = ageSeconds;
    if (seconds < 60) return `updated ${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `updated ${minutes}m ago`;
    return `updated ${Math.floor(minutes / 60)}h ago`;
  });

  // The genuine cold-start case only: cached data, and no live snapshot has
  // arrived yet. A poll that merely failed (rate limited, offline) no longer
  // touches `stale` at all, so it never lands here — the footer just keeps
  // counting up the age of the last good snapshot instead.
  const showingCachedData = $derived(!hasLiveSnapshot && ($snapshot?.snapshot.stale ?? false));
  const footerIsWarning = $derived(ageSeconds > STALE_WARNING_SECS);

  // The session quota gets the full-width meter; everything else — any
  // number of weekly windows, present or future — goes in the ring grid.
  // Never hard-code which quotas exist: a new model's weekly window must
  // appear here with no code change, which is the whole point of consuming
  // `limits[]` in the first place.
  const quotas = $derived($snapshot?.snapshot.quotas ?? []);
  const session = $derived(quotas.find((q) => q.id === "session") ?? null);
  const rings = $derived(quotas.filter((q) => q.id !== "session"));

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
      const height = Math.min(
        MAX_HEIGHT,
        Math.max(MIN_HEIGHT, Math.ceil(mainEl.scrollHeight)),
      );
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
    <button title="Refresh now" onclick={() => invoke("refresh_now")}>
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
    <footer class:stale={footerIsWarning}>{age}</footer>
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

  footer { padding: 10px 14px 6px; font-size: 11px; color: var(--fg-muted); }
  footer.stale { color: var(--warning); }
</style>

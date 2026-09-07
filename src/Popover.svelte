<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import QuotaCard from "./lib/QuotaCard.svelte";
  import { now, snapshot } from "./lib/stores";

  const STALE_WARNING_SECS = 5 * 60;

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
  const footerIsWarning = $derived(showingCachedData || ageSeconds > STALE_WARNING_SECS);
</script>

<main>
  <header>
    <div class="who">
      <div class="name">{$snapshot?.profile?.displayName ?? "Claude usage"}</div>
      {#if $snapshot?.profile?.planLabel}
        <div class="plan">{$snapshot.profile.planLabel}</div>
      {/if}
    </div>
    <button title="Refresh now" onclick={() => invoke("refresh_now")}>⟳</button>
    <button title="Settings" onclick={() => invoke("open_settings")}>⚙</button>
  </header>

  {#if $snapshot?.signedOut}
    <p class="empty">Sign in to Claude Code to see your usage.</p>
  {:else if !$snapshot}
    <p class="empty">Loading…</p>
  {:else}
    {#each $snapshot.snapshot.quotas as quota (quota.id)}
      <QuotaCard {quota} now={$now} />
    {/each}
    <footer class:stale={footerIsWarning}>
      {showingCachedData ? "showing cached data" : age}
    </footer>
  {/if}
</main>

<style>
  main {
    background: var(--bg);
    backdrop-filter: blur(30px);
    border-radius: 12px;
    padding: 6px;
    height: 100vh;
    overflow-y: auto;
  }
  header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px 10px;
    border-bottom: 1px solid var(--hairline);
    margin-bottom: 4px;
  }
  .who { flex: 1; min-width: 0; }
  .name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .plan { font-size: 11px; color: var(--fg-muted); }
  button {
    background: none;
    border: none;
    color: var(--fg-muted);
    font-size: 14px;
    cursor: pointer;
    padding: 4px;
    border-radius: 5px;
  }
  button:hover { color: var(--fg); background: var(--track); }
  .empty { padding: 24px 14px; text-align: center; color: var(--fg-muted); }
  footer { padding: 8px 14px 4px; font-size: 11px; color: var(--fg-muted); }
  footer.stale { color: var(--warning); }
</style>

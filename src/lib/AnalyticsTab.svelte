<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Callout from "./Callout.svelte";
  import {
    coveredRange,
    formatCost,
    formatTokens,
    modelLabel,
    shortProject,
    unpricedModels,
    type Summary,
  } from "./analytics";

  // The parent re-measures the popover's height whenever this fires; the tab's
  // content arrives well after the window has been sized for the Limits tab.
  let { onloaded }: { onloaded?: () => void } = $props();

  // Three states, not two: no answer yet, an answer, and a scan that failed.
  // Leaving `summary` null on failure would leave "Scanning…" on screen
  // forever, which is a worse lie than an error.
  let summary = $state<Summary | null>(null);
  let error = $state<string | null>(null);

  invoke<Summary>("analytics_summary")
    .then((s) => (summary = s))
    .catch((e) => (error = String(e)))
    .finally(() => onloaded?.());

  // Everything here is a floor rather than a total when some of the tokens
  // ran on a model this build has no price for. `formatCost` puts the "+" on
  // those figures and the callout below says why.
  const missing = $derived(summary ? unpricedModels(summary.byModel) : []);

  // A total with no period stated invites being read as a lifetime one. What
  // this actually covers is whatever transcript history is still on disk.
  const covered = $derived(summary ? coveredRange(summary.byDay) : null);
</script>

{#if error}
  <p class="empty">Could not read local usage: {error}</p>
{:else if !summary}
  <p class="empty">Scanning transcripts…</p>
{:else if summary.totalTokens === 0}
  <p class="empty">No local usage found in ~/.claude/projects.</p>
{:else}
  <div class="totals">
    <span>{formatTokens(summary.totalTokens)} tokens</span>
    <span>{formatCost(summary.totalCost, summary.unpricedTokens)}</span>
  </div>
  <p class="caveat">
    {#if covered}
      Covers {covered.first} to {covered.last} — every transcript still on disk;
      Claude Code prunes older ones.
    {/if}
    Estimate from API list prices applied to subscription usage, not a bill.
  </p>

  {#if missing.length > 0}
    <div class="notice">
      <Callout tone="warning">
        {formatTokens(summary.unpricedTokens)} tokens ran on
        {missing.length === 1 ? "a model" : `${missing.length} models`} with no price
        in this build, so they are counted above but cost nothing here:
        {missing.map((bucket) => modelLabel(bucket.name)).join(", ")}. Figures that
        leave them out are marked&nbsp;+.
      </Callout>
    </div>
  {/if}

  <!-- `unpricedTokens === tokens` reads as "wholly unpriced" only because
       `summarize` never emits a bucket that spent nothing; a zero-token row
       would satisfy it with 0 === 0 and be labelled a warning about nothing. -->
  <h4>By model</h4>
  {#each summary.byModel as bucket (bucket.name)}
    <div class="row" class:unpriced={bucket.unpricedTokens === bucket.tokens}>
      <span title={bucket.name}>{modelLabel(bucket.name)}</span>
      <span class="tokens">{formatTokens(bucket.tokens)}</span>
      {#if bucket.unpricedTokens === bucket.tokens}
        <span class="cost" title="No price for this model in this build"
          >not priced</span
        >
      {:else}
        <span class="cost">{formatCost(bucket.cost, bucket.unpricedTokens)}</span>
      {/if}
    </div>
  {/each}

  <h4>By project</h4>
  {#each summary.byProject.slice(0, 8) as bucket (bucket.name)}
    <div class="row">
      <span title={bucket.name}>{shortProject(bucket.name)}</span>
      <span class="tokens">{formatTokens(bucket.tokens)}</span>
      <span class="cost">{formatCost(bucket.cost, bucket.unpricedTokens)}</span>
    </div>
  {/each}

  <!-- Not "Last 14 days": these are the newest 14 days that *had* usage, and
       any idle day between them means they span more than fourteen. -->
  <h4>Recent days</h4>
  {#each summary.byDay.slice(0, 14) as bucket (bucket.name)}
    <div class="row">
      <span>{bucket.name}</span>
      <span class="tokens">{formatTokens(bucket.tokens)}</span>
      <span class="cost">{formatCost(bucket.cost, bucket.unpricedTokens)}</span>
    </div>
  {/each}
{/if}

<style>
  .totals {
    display: flex;
    justify-content: space-between;
    padding: 2px 14px;
    font-size: 16px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .caveat {
    padding: 0 14px;
    margin: 4px 0 10px;
    font-size: 11px;
    line-height: 1.45;
    color: var(--fg-muted);
  }
  .notice {
    padding: 0 10px;
    margin-bottom: 10px;
  }
  .empty {
    padding: 24px 14px;
    text-align: center;
    color: var(--fg-muted);
  }
  h4 {
    margin: 12px 14px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-muted);
  }
  .row {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    padding: 3px 14px;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .row span:first-child {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tokens {
    color: var(--fg-muted);
    flex: none;
  }
  /* The column the eye goes to, so it stays at full contrast — except on a
     row that has no price to show, where "not priced" is a caveat rather than
     a figure and must not be mistaken for one. */
  .cost {
    color: var(--fg);
    flex: none;
    min-width: 58px;
    text-align: right;
  }
  .row.unpriced .cost {
    color: var(--high);
    font-style: italic;
  }
</style>

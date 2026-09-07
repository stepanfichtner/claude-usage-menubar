<script lang="ts">
  import { formatLong, formatResetTime } from "./countdown";
  import { severityColor } from "./severity";
  import type { Quota } from "./types";

  let { quota, now }: { quota: Quota; now: Date } = $props();

  const pct = $derived(Math.min(100, Math.max(0, quota.percent)));
  const colour = $derived(severityColor(quota.severity));
</script>

<div class="card" class:active={quota.isActive}>
  <div class="row">
    <span class="label">{quota.label}</span>
    <span class="pct">{Math.round(quota.percent)}%</span>
  </div>
  <div class="track">
    <div class="fill" style="width: {pct}%; background: {colour}"></div>
  </div>
  <div class="meta">
    {#if quota.resetsAt}
      <span>{formatResetTime(quota.resetsAt)}</span>
      <span>{formatLong(quota.resetsAt, now)} left</span>
    {:else}
      <span>—</span>
    {/if}
  </div>
</div>

<style>
  .card {
    padding: 12px 14px;
    margin: 0 8px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .card.active { border-color: var(--fg-muted); }
  .row { display: flex; justify-content: space-between; align-items: baseline; }
  .label { font-weight: 500; color: var(--fg); }
  .pct { font-variant-numeric: tabular-nums; font-size: 15px; font-weight: 600; color: var(--fg); }
  .track {
    height: 6px;
    margin: 9px 0 7px;
    border-radius: 999px;
    background: var(--track);
    overflow: hidden;
  }
  .fill { height: 100%; border-radius: 999px; transition: width 0.4s ease; }
  .meta {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    font-size: 11px;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
</style>

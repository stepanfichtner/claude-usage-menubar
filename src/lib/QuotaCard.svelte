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
      {formatResetTime(quota.resetsAt)} · in {formatLong(quota.resetsAt, now)}
    {:else}
      —
    {/if}
  </div>
</div>

<style>
  .card { padding: 10px 14px; border-radius: 8px; }
  .card.active { background: var(--active-glow); }
  .row { display: flex; justify-content: space-between; align-items: baseline; }
  .label { font-weight: 500; }
  .pct { font-variant-numeric: tabular-nums; font-size: 15px; font-weight: 600; }
  .track {
    height: 6px;
    margin: 7px 0 5px;
    border-radius: 3px;
    background: var(--track);
    overflow: hidden;
  }
  .fill { height: 100%; border-radius: 3px; transition: width 0.4s ease; }
  .meta { font-size: 11px; color: var(--fg-muted); font-variant-numeric: tabular-nums; }
</style>

<script lang="ts">
  import { severityColor } from "./severity";
  import type { Severity } from "./types";

  let { percent, severity }: { percent: number; severity: Severity } = $props();

  const R = 42;
  const CIRC = 2 * Math.PI * R;
  const clamped = $derived(Math.min(100, Math.max(0, percent)));
  const offset = $derived(CIRC * (1 - clamped / 100));
</script>

<div class="ring">
  <svg viewBox="0 0 100 100" aria-hidden="true">
    <circle cx="50" cy="50" r={R} class="track" />
    <circle
      cx="50" cy="50" r={R}
      class="fill"
      style="stroke: {severityColor(severity)}; stroke-dasharray: {CIRC}; stroke-dashoffset: {offset}"
    />
  </svg>
  <span class="value">{Math.round(percent)}%</span>
</div>

<style>
  .ring { position: relative; width: 84px; height: 84px; }
  svg { width: 100%; height: 100%; transform: rotate(-90deg); }
  circle { fill: none; stroke-width: 8; }
  .track { stroke: var(--track); }
  .fill { stroke-linecap: round; transition: stroke-dashoffset 0.4s ease; }
  .value {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 17px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--fg);
  }
</style>

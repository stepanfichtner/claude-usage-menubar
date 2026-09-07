<script lang="ts">
  let { tone = "muted", children }: { tone?: "muted" | "warning"; children: import("svelte").Snippet } =
    $props();
</script>

<div class="callout" class:warning={tone === "warning"}>
  <span class="dot"></span>
  <span class="text">{@render children()}</span>
</div>

<style>
  .callout {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-size: 11px;
    color: var(--fg-muted);
  }
  .dot { width: 6px; height: 6px; border-radius: 50%; background: var(--fg-muted); flex: none; }
  .warning .dot { background: var(--high); }
</style>

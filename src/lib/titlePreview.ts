import { formatCompact } from "./countdown";
import type { TitleEntry } from "./titleEntries";
import type { Quota } from "./types";

/**
 * Mirrors `tray::render_title` in the Rust crate (src-tauri/src/tray.rs):
 * same quotas, same filter (an entry with nothing to show, or whose quota
 * isn't in this snapshot, drops out entirely rather than leaving a blank
 * segment), same "percent · countdown" segment and two-space join between
 * entries. Keep the two in step.
 */
export function renderTitle(quotas: Quota[], entries: TitleEntry[], now: Date): string {
  return entries
    .map((entry) => {
      const quota = quotas.find((q) => q.id === entry.quotaId);
      if (!quota) return null;
      const parts: string[] = [];
      if (entry.showPercent) parts.push(`${Math.round(quota.percent)}%`);
      if (entry.showCountdown && quota.resetsAt) {
        parts.push(formatCompact(quota.resetsAt, now));
      }
      return parts.length > 0 ? parts.join(" · ") : null;
    })
    .filter((segment): segment is string => segment !== null)
    .join("  ");
}

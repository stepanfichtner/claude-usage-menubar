import { formatCompact } from "./countdown";
import type { TitleEntry } from "./titleEntries";
import type { Quota, TitleSeparator } from "./types";

/**
 * Mirrors `TitleSeparator::glyph` (src-tauri/src/tray/render.rs), keyed by
 * the same names the store holds. Spacing is part of each value, exactly as
 * it is there: the mark and the room around it are one decision, and a menu
 * bar that spaced them differently from the real title would make this
 * preview a lie about the very thing it is previewing.
 */
const SEPARATOR_GLYPHS: Record<TitleSeparator, string> = {
  space: "  ",
  pipe: " | ",
  diamond: " ◆ ",
  slash: " / ",
};

/**
 * What goes between two segments. The fallback mirrors the Rust side's
 * `TitleSeparator::Unknown`, which renders as the default rather than as
 * nothing: `Settings::sanitized` should have replaced any such name before it
 * reached this window, and if one ever does arrive the preview stays honest
 * about what the menu bar will draw — the same title, unchanged.
 */
export function separatorGlyph(separator: TitleSeparator): string {
  return SEPARATOR_GLYPHS[separator] ?? SEPARATOR_GLYPHS.space;
}

/**
 * Mirrors `tray::render_title` in the Rust crate
 * (src-tauri/src/tray/render.rs): same quotas, same filter (an entry with
 * nothing to show, or whose quota isn't in this snapshot, drops out entirely
 * rather than leaving a blank segment or a separator with nothing beside it),
 * same "percent · countdown" segment, and the same `separator` between one
 * entry and the next. Keep the two in step.
 */
export function renderTitle(
  quotas: Quota[],
  entries: TitleEntry[],
  separator: TitleSeparator,
  now: Date,
): string {
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
    .join(separatorGlyph(separator));
}

export type Severity = "normal" | "warning" | "high" | "critical";

export interface Quota {
  id: string;
  label: string;
  percent: number;
  severity: Severity;
  /** RFC 3339 timestamp, or null when the server did not supply one. */
  resetsAt: string | null;
  isActive: boolean;
}

export interface UsageSnapshot {
  quotas: Quota[];
  fetchedAt: string;
  stale: boolean;
}

export interface Profile {
  displayName: string;
  planLabel: string;
}

/**
 * Mirrors `tray::TitleSeparator` (src-tauri/src/tray/render.rs): the names
 * `settings.json` stores, not the marks themselves — `separatorGlyph` in
 * `titlePreview.ts` is the mirror of what each one draws.
 *
 * The Rust enum has one case more than this union. A name neither build
 * knows deserializes to `Unknown` there, and `Settings::sanitized` replaces
 * it on the way out of the store, so it cannot reach this window and there is
 * nothing here to represent it.
 */
export type TitleSeparator = "space" | "pipe" | "dash" | "slash";

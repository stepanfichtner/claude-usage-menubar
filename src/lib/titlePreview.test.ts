import { describe, expect, it } from "vitest";
import { renderTitle } from "./titlePreview";
import type { TitleEntry } from "./titleEntries";
import type { Quota } from "./types";

// Mirrors `tray::tests::*` in the Rust crate (src-tauri/src/tray.rs,
// `render_title`'s own case table) — same cases, same expected strings, so
// the two implementations can't quietly drift apart.

const now = new Date("2026-09-07T09:00:00Z");
const ahead = (minutes: number) => new Date(now.getTime() + minutes * 60_000).toISOString();

function quota(id: string, percent: number, minutesAhead: number): Quota {
  return {
    id,
    label: `Label ${id}`,
    percent,
    severity: "normal",
    resetsAt: ahead(minutesAhead),
    isActive: false,
  };
}

const defaultEntries: TitleEntry[] = [
  { quotaId: "session", showPercent: true, showCountdown: true },
  { quotaId: "weekly_all", showPercent: true, showCountdown: true },
];

describe("renderTitle", () => {
  // Mirrors `the_default_title_shows_session_and_weekly_all`.
  it("shows session and weekly_all by default", () => {
    const quotas = [quota("session", 20, 238), quota("weekly_all", 2, 6 * 24 * 60)];
    expect(renderTitle(quotas, defaultEntries, now)).toBe("20% · 3h58m  2% · 6d0h");
  });

  // Mirrors `entries_can_drop_the_countdown`.
  it("can drop the countdown", () => {
    const quotas = [quota("session", 20, 238)];
    const entries: TitleEntry[] = [
      { quotaId: "session", showPercent: true, showCountdown: false },
    ];
    expect(renderTitle(quotas, entries, now)).toBe("20%");
  });

  // Mirrors `entries_naming_an_absent_quota_are_skipped_silently`.
  it("silently skips an entry naming an absent quota", () => {
    const quotas = [quota("session", 20, 238)];
    expect(renderTitle(quotas, defaultEntries, now)).toBe("20% · 3h58m");
  });

  // Mirrors `an_empty_entry_list_renders_nothing`.
  it("renders nothing for an empty entry list", () => {
    const quotas = [quota("session", 20, 238)];
    expect(renderTitle(quotas, [], now)).toBe("");
  });

  // Mirrors `a_quota_without_a_reset_time_omits_the_countdown`.
  it("omits the countdown for a quota with no reset time", () => {
    const q = quota("weekly:Fable", 0, 0);
    q.resetsAt = null;
    const entries: TitleEntry[] = [
      { quotaId: "weekly:Fable", showPercent: true, showCountdown: true },
    ];
    expect(renderTitle([q], entries, now)).toBe("0%");
  });
});

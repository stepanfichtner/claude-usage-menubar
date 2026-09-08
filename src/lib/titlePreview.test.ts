import { describe, expect, it } from "vitest";
import { renderTitle, separatorGlyph } from "./titlePreview";
import type { TitleEntry } from "./titleEntries";
import type { Quota, TitleSeparator } from "./types";

// Mirrors `tray::render::tests::*` in the Rust crate
// (src-tauri/src/tray/render.rs, `render_title`'s own case table) — same
// cases, same expected strings, so the two implementations can't quietly
// drift apart. The one case with no mirror here is
// `separator_names_round_trip_through_their_stored_form`, which is about how
// Rust writes the choice into settings.json; nothing on this side serializes
// it.

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

// Every name, and a `Record` rather than an array literal on purpose: a fifth
// member of `TitleSeparator` fails to type-check here until it is listed,
// which is this side's mirror of `TitleSeparator::offered()` walking an
// exhaustive `match`. An array would go on compiling while the new choice sat
// untested — the flaw this replaced.
const EVERY_SEPARATOR: Record<TitleSeparator, true> = {
  space: true,
  pipe: true,
  dash: true,
  slash: true,
};
const everySeparator = Object.keys(EVERY_SEPARATOR) as TitleSeparator[];

describe("renderTitle", () => {
  // Mirrors `the_default_title_shows_session_and_weekly_all`.
  it("shows session and weekly_all by default", () => {
    const quotas = [quota("session", 20, 238), quota("weekly_all", 2, 6 * 24 * 60)];
    expect(renderTitle(quotas, defaultEntries, "space", now)).toBe("20% · 3h58m  2% · 6d0h");
  });

  // Mirrors `entries_can_drop_the_countdown`.
  it("can drop the countdown", () => {
    const quotas = [quota("session", 20, 238)];
    const entries: TitleEntry[] = [
      { quotaId: "session", showPercent: true, showCountdown: false },
    ];
    expect(renderTitle(quotas, entries, "space", now)).toBe("20%");
  });

  // Mirrors `entries_naming_an_absent_quota_are_skipped_silently`.
  it("silently skips an entry naming an absent quota", () => {
    const quotas = [quota("session", 20, 238)];
    expect(renderTitle(quotas, defaultEntries, "space", now)).toBe("20% · 3h58m");
  });

  // Mirrors `an_empty_entry_list_renders_nothing`.
  it("renders nothing for an empty entry list", () => {
    const quotas = [quota("session", 20, 238)];
    expect(renderTitle(quotas, [], "space", now)).toBe("");
  });

  // Mirrors `a_quota_without_a_reset_time_omits_the_countdown`.
  it("omits the countdown for a quota with no reset time", () => {
    const q = quota("weekly:Fable", 0, 0);
    q.resetsAt = null;
    const entries: TitleEntry[] = [
      { quotaId: "weekly:Fable", showPercent: true, showCountdown: true },
    ];
    expect(renderTitle([q], entries, "space", now)).toBe("0%");
  });

  // Mirrors `every_separator_joins_each_pair_of_entries_with_its_own_glyph`,
  // expected string for expected string: the three entries of the complaint
  // this setting answers, under every choice. Three rather than two because a
  // separator inserted once, after the first segment only, would be
  // indistinguishable from one joining every pair with just two.
  it("puts the chosen separator between every pair of entries", () => {
    const quotas = [
      quota("session", 24, 241),
      quota("weekly_all", 33, 6 * 24 * 60),
      quota("weekly:Fable", 3, 6 * 24 * 60),
    ];
    const entries: TitleEntry[] = [
      { quotaId: "session", showPercent: true, showCountdown: true },
      { quotaId: "weekly_all", showPercent: true, showCountdown: false },
      { quotaId: "weekly:Fable", showPercent: true, showCountdown: false },
    ];
    const expected: Record<TitleSeparator, string> = {
      space: "24% · 4h1m  33%  3%",
      pipe: "24% · 4h1m | 33% | 3%",
      dash: "24% · 4h1m – 33% – 3%",
      slash: "24% · 4h1m / 33% / 3%",
    };

    for (const separator of everySeparator) {
      expect(renderTitle(quotas, entries, separator, now), separator).toBe(expected[separator]);
    }
  });

  // Mirrors `nothing_is_separated_from_nothing`.
  it("shows no separator when there is nothing to separate", () => {
    const quotas = [quota("session", 20, 238)];
    const one: TitleEntry[] = [{ quotaId: "session", showPercent: true, showCountdown: true }];

    for (const separator of everySeparator) {
      expect(renderTitle(quotas, one, separator, now), separator).toBe("20% · 3h58m");
      expect(renderTitle(quotas, [], separator, now), separator).toBe("");
    }
  });

  // Mirrors `an_entry_that_shows_nothing_leaves_no_separator_behind`: what a
  // filter running after the join would leave behind — `24% | ` for a quota
  // this snapshot does not carry, `24% |  | 3%` for an entry with neither box
  // ticked.
  it("leaves no separator behind for an entry that shows nothing", () => {
    const quotas = [quota("session", 24, 241), quota("weekly:Fable", 3, 60)];

    const absent: TitleEntry[] = [
      { quotaId: "session", showPercent: true, showCountdown: false },
      { quotaId: "weekly_all", showPercent: true, showCountdown: true },
    ];
    expect(renderTitle(quotas, absent, "pipe", now)).toBe("24%");

    const unticked: TitleEntry[] = [
      { quotaId: "session", showPercent: true, showCountdown: false },
      { quotaId: "weekly_all", showPercent: false, showCountdown: false },
      { quotaId: "weekly:Fable", showPercent: true, showCountdown: false },
    ];
    expect(renderTitle(quotas, unticked, "pipe", now)).toBe("24% | 3%");
  });

  // Mirrors `a_separator_name_this_build_does_not_know_is_not_an_error`. The
  // cast is the point: the type says this cannot happen, and the value comes
  // over IPC from a store that a newer build may have written, so the
  // fallback has to hold when it does.
  it("falls back to the default join for a name it does not know", () => {
    const quotas = [quota("session", 20, 238), quota("weekly_all", 2, 6 * 24 * 60)];
    const unknown = "arrow" as unknown as TitleSeparator;
    expect(renderTitle(quotas, defaultEntries, unknown, now)).toBe("20% · 3h58m  2% · 6d0h");
  });
});

describe("separatorGlyph", () => {
  // Mirrors `no_offered_separator_can_be_mistaken_for_the_dot_inside_a_segment`.
  it("offers no separator that could be mistaken for the dot inside a segment", () => {
    for (const separator of everySeparator.filter((s) => s !== "space")) {
      const glyph = separatorGlyph(separator);
      expect(glyph, `${separator} uses the intra-segment dot`).not.toContain("·");
      expect(glyph, `${separator} uses a bullet, that dot at twice the size`).not.toContain("•");
      expect(glyph.trim(), `${separator} has nothing to see`).not.toBe("");
    }
    expect(separatorGlyph("space"), "the default is 0.1.0's two spaces, exactly").toBe("  ");
  });
});

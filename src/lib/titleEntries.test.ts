import { describe, expect, it } from "vitest";
import { mergeTitleEntries } from "./titleEntries";
import type { Quota } from "./types";

function quota(id: string): Quota {
  return { id, label: id, percent: 0, severity: "normal", resetsAt: null, isActive: true };
}

describe("mergeTitleEntries", () => {
  // R43: an empty (or absent) snapshot must never be read as "no quota
  // belongs in the menu bar any more" — this is the first-run and
  // signed-out-blip path that previously wiped the user's configuration.
  it("leaves existing entries untouched when quotas is empty", () => {
    const existing = [
      { quotaId: "session", showPercent: true, showCountdown: true },
      { quotaId: "weekly_all", showPercent: true, showCountdown: true },
    ];
    expect(mergeTitleEntries(existing, [])).toEqual(existing);
  });

  it("appends a quota that has no entry yet, unchecked", () => {
    const existing = [{ quotaId: "session", showPercent: true, showCountdown: true }];
    const merged = mergeTitleEntries(existing, [quota("session"), quota("fable")]);
    expect(merged).toEqual([
      { quotaId: "session", showPercent: true, showCountdown: true },
      { quotaId: "fable", showPercent: false, showCountdown: false },
    ]);
  });

  it("keeps an existing entry's flags rather than resetting them", () => {
    const existing = [{ quotaId: "session", showPercent: false, showCountdown: true }];
    const merged = mergeTitleEntries(existing, [quota("session")]);
    expect(merged).toEqual(existing);
  });

  // A union, not a replacement: a quota temporarily missing from a snapshot
  // (rate limited, a plan change, a partial response) must not cost the
  // user their configuration for it.
  it("keeps an entry whose quota is absent from the current snapshot", () => {
    const existing = [
      { quotaId: "session", showPercent: true, showCountdown: true },
      { quotaId: "weekly_all", showPercent: true, showCountdown: true },
    ];
    const merged = mergeTitleEntries(existing, [quota("session")]);
    expect(merged).toEqual(existing);
  });
});

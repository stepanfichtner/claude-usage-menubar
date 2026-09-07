import { describe, expect, it } from "vitest";
import {
  RETIREMENT_MISSES,
  mergeTitleEntries,
  retireAbsentEntries,
  type Absences,
  type SnapshotStanding,
  type TitleEntry,
} from "./titleEntries";
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

describe("retireAbsentEntries", () => {
  const session: TitleEntry = { quotaId: "session", showPercent: true, showCountdown: true };
  const fable: TitleEntry = { quotaId: "weekly:Fable", showPercent: true, showCountdown: false };
  const entries = [session, fable];

  function good(...ids: string[]): SnapshotStanding {
    return { quotas: ids.map(quota), stale: false, signedOut: false };
  }

  /** Feed the same snapshot `times` over, threading the tally through. */
  function poll(start: TitleEntry[], standing: SnapshotStanding | null, times: number) {
    let entries = start;
    let absences: Absences = {};
    for (let i = 0; i < times; i++) {
      ({ entries, absences } = retireAbsentEntries(entries, standing, absences));
    }
    return { entries, absences };
  }

  // The counts below are written out rather than derived from
  // `RETIREMENT_MISSES`: derived, they would follow a changed constant
  // instead of pinning it, and the pair of them is the whole point — two
  // misses keep, three retire. Lowering the constant to 1 (removing the
  // sustained requirement entirely) has to fail a test that says so.
  it("takes three consecutive misses to retire an entry", () => {
    expect(RETIREMENT_MISSES).toBe(3);
    expect(poll(entries, good("session"), 2).entries).toEqual(entries);
    expect(poll(entries, good("session"), 3).entries).toEqual([session]);
  });

  // One snapshot lacking a quota is never enough — the case the R43 comment
  // calls "a transient blip".
  it("keeps an entry that a single good snapshot omitted", () => {
    expect(retireAbsentEntries(entries, good("session"), {}).entries).toEqual(entries);
  });

  // `normalize` derives a scoped weekly's id from `scope.model.display_name`
  // and falls back to `weekly:scoped` when the server omits it, so one
  // successful response can genuinely lack an id it will report again next
  // poll. The tally has to reset, not merely pause.
  it("forgets the tally when the quota comes back", () => {
    let { entries: kept, absences } = poll(entries, good("session"), 2);
    ({ entries: kept, absences } = retireAbsentEntries(
      kept,
      good("session", "weekly:Fable"),
      absences,
    ));
    expect(absences).toEqual({});
    // The counter restarted, so two more misses are not enough either — a
    // tally that merely paused would have retired on the first of these.
    for (let i = 0; i < 2; i++) {
      ({ entries: kept, absences } = retireAbsentEntries(kept, good("session"), absences));
    }
    expect(kept).toEqual(entries);
  });

  // R43. Each of these is a snapshot that says nothing about which quotas
  // exist, and each would be a way to wipe the user's configuration. The
  // last two carry quotas: a snapshot can be untrustworthy and non-empty at
  // the same time, so "non-empty" alone is not the gate.
  it.each([
    ["no snapshot at all", null],
    ["the cached snapshot replayed at launch", { quotas: [], stale: true, signedOut: false }],
    ["a signed-out snapshot", { quotas: [], stale: false, signedOut: true }],
    ["an empty snapshot", { quotas: [], stale: false, signedOut: false }],
    [
      "a stale snapshot that does list quotas",
      { quotas: [quota("session")], stale: true, signedOut: false },
    ],
    [
      "a signed-out snapshot that does list quotas",
      { quotas: [quota("session")], stale: false, signedOut: true },
    ],
  ] as [string, SnapshotStanding | null][])("never retires on %s", (_label, standing) => {
    const { entries: kept, absences } = poll(entries, standing, 9);
    expect(kept).toBe(entries);
    expect(absences).toEqual({});
  });

  // The `$effect` in Settings.svelte writes its own result back into
  // `settings.titleEntries`. A fresh array every poll would re-trigger it.
  it("returns the same array when nothing is retired", () => {
    expect(retireAbsentEntries(entries, good("session"), {}).entries).toBe(entries);
  });
});

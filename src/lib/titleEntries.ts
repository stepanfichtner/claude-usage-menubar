import type { Quota } from "./types";

export interface TitleEntry {
  quotaId: string;
  showPercent: boolean;
  showCountdown: boolean;
}

/**
 * A union, never a replacement (R43). Every existing entry survives
 * untouched, even one whose quota is absent from `quotas` — a quota can
 * disappear from a snapshot for reasons that have nothing to do with the
 * user's intent (signed out, a transient blip, an empty first-run
 * snapshot), and the Rust side's `render_title` already skips entries it
 * cannot match to a live quota, so a preserved-but-unrenderable entry costs
 * nothing. A quota with no existing entry gets one new, unchecked entry
 * appended.
 *
 * When `quotas` is empty there is nothing to merge from — return `existing`
 * unchanged rather than let "no quota matched" be reinterpreted as "no
 * entries belong here any more".
 */
export function mergeTitleEntries(existing: TitleEntry[], quotas: Quota[]): TitleEntry[] {
  if (quotas.length === 0) return existing;

  const known = new Set(existing.map((e) => e.quotaId));
  const additions: TitleEntry[] = quotas
    .filter((q) => !known.has(q.id))
    .map((q) => ({ quotaId: q.id, showPercent: false, showCountdown: false }));

  return additions.length === 0 ? existing : [...existing, ...additions];
}

/**
 * How many consecutive trustworthy snapshots must omit a quota before its
 * entry is retired.
 *
 * The poll interval floor is 60s and the longest preset is 600s
 * (`MIN_POLL_INTERVAL_SECS`, `POLL_INTERVAL_PRESETS`), so three misses is
 * between three and thirty minutes of continuous absence. A genuinely
 * retired quota — a plan change — is absent forever and clears that in one
 * poll cycle; nothing this app has been seen to do transiently lasts three
 * consecutive successful live fetches.
 */
export const RETIREMENT_MISSES = 3;

/**
 * Just the parts of a `SnapshotEvent` that decide whether it is trustworthy
 * enough to count an absence against. Structural rather than imported from
 * `stores.ts`, which opens a Tauri event listener at module scope.
 */
export interface SnapshotStanding {
  quotas: Quota[];
  /** True for the snapshot replayed from cache at launch. */
  stale: boolean;
  signedOut: boolean;
}

/** Absence tallies, keyed by quota id. Absent key means "seen recently". */
export type Absences = Readonly<Record<string, number>>;

export interface Retirement {
  entries: TitleEntry[];
  absences: Absences;
}

/**
 * The other half of R43: an entry whose quota is genuinely gone should not
 * linger in the settings list forever, showing a raw quota id (`labelFor`
 * has no live quota to get a label from) above two checkboxes that cannot
 * affect anything (`render_title` skips entries it cannot match).
 *
 * Removal is deliberately hard to trigger, because the wipe R43 prevents is
 * far more expensive than the lingering row. Two independent guards:
 *
 * 1. Only a *trustworthy* snapshot counts. A snapshot that is `stale` (the
 *    cached one replayed at launch, never confirmed by a live fetch),
 *    `signedOut`, or empty says nothing about which quotas exist — those are
 *    exactly the paths that used to wipe the user's configuration. On one of
 *    those this returns everything untouched, and does not even count.
 * 2. Absence must be *sustained*: `RETIREMENT_MISSES` consecutive
 *    trustworthy snapshots, reset to zero the moment the quota reappears.
 *    One snapshot lacking a quota is never enough, and it is not a
 *    hypothetical: `normalize` derives a scoped weekly's id from the
 *    server's `scope.model.display_name` and falls back to `weekly:scoped`
 *    when it is missing, so a single response that omits that one field
 *    turns `weekly:Fable` into a different quota id and back again.
 *
 * `entries` is returned by identity when nothing is retired, so the caller's
 * `$effect` does not re-run on its own write.
 */
export function retireAbsentEntries(
  entries: TitleEntry[],
  standing: SnapshotStanding | null | undefined,
  absences: Absences,
): Retirement {
  if (!standing || standing.stale || standing.signedOut || standing.quotas.length === 0) {
    return { entries, absences };
  }

  const live = new Set(standing.quotas.map((q) => q.id));
  const next: Record<string, number> = {};
  for (const entry of entries) {
    if (!live.has(entry.quotaId)) {
      next[entry.quotaId] = (absences[entry.quotaId] ?? 0) + 1;
    }
  }

  const retired = entries.filter((e) => (next[e.quotaId] ?? 0) >= RETIREMENT_MISSES);
  if (retired.length === 0) return { entries, absences: next };

  const gone = new Set(retired.map((e) => e.quotaId));
  for (const id of gone) delete next[id];
  return { entries: entries.filter((e) => !gone.has(e.quotaId)), absences: next };
}

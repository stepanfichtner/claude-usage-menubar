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

/**
 * The Usage tab's presentation logic, kept out of `AnalyticsTab.svelte` so the
 * vitest suite can reach it — the same split as `panel.ts` and `freshness.ts`.
 *
 * Everything here exists to keep one promise: the tab must never show a dollar
 * figure that reads like a complete total when it is not one. A quota
 * percentage that is wrong looks wrong; a wrong dollar figure looks exactly
 * like a right one.
 */

/** Mirrors `analytics::Bucket` (src-tauri/src/analytics/mod.rs). */
export interface Bucket {
  name: string;
  tokens: number;
  /** The priced part only. A floor when `unpricedTokens` is above zero. */
  cost: number;
  /** How many of `tokens` ran on a model this build cannot price. */
  unpricedTokens: number;
}

/** Mirrors `analytics::Summary`. */
export interface Summary {
  byModel: Bucket[];
  byProject: Bucket[];
  byDay: Bucket[];
  totalCost: number;
  totalTokens: number;
  unpricedTokens: number;
}

/**
 * A token count at a glance.
 *
 * Not simply `(n / 1e6).toFixed(1) + "M"`, in either direction. Below a
 * million that renders every real figure as "0.0M", so a lightly used machine
 * reads as broken rather than as quiet. Above a billion — which a year of
 * daily use reaches — it renders as "1172.3M", four digits of mantissa that
 * nobody can read at a glance.
 *
 * The thresholds are 999,500 and 999,500,000 rather than the round numbers
 * because they are where the *smaller* unit starts rounding to a four-digit
 * mantissa: 999,999 tokens is "1000K" at the round threshold and "1.0M" here,
 * and the second is the same number said better.
 */
export function formatTokens(tokens: number): string {
  if (tokens >= 999_500_000) return `${(tokens / 1_000_000_000).toFixed(2)}B`;
  if (tokens >= 999_500) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return `${tokens}`;
}

/**
 * A cost, said only as precisely as it is known.
 *
 * Two roundings would otherwise state more than the number supports, and both
 * are the reason this is a function rather than a template string:
 *
 * - `$12.34` claims to be what those tokens cost. When some of them ran on a
 *   model this build has no price for, the only true statement is that they
 *   cost at least that, so the figure carries a trailing `+`.
 * - `$0.00` claims the bucket cost nothing, which is what a wholly unpriced
 *   model would be shown as. A priced bucket that spent a third of a cent did
 *   not cost nothing, so anything under half a cent reads `<$0.01`.
 */
export function formatCost(cost: number, unpricedTokens = 0): string {
  const marker = unpricedTokens > 0 ? "+" : "";
  // "$0.00" is reserved for a bucket that genuinely cost nothing, which is
  // what a wholly unpriced model would otherwise be shown as. A priced bucket
  // that spent a third of a cent did not cost nothing, and rounding it down
  // makes real spend indistinguishable from free — the same error as pricing
  // an unknown model at zero, one step further along.
  if (cost > 0 && cost < 0.005) return `<$0.01${marker}`;
  // Grouped by hand rather than through `toLocaleString`, whose separator
  // follows the machine's locale — a figure this app formats itself reads the
  // same in a screenshot as it does in a test.
  const [whole, decimals] = cost.toFixed(2).split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `$${grouped}.${decimals}${marker}`;
}

/**
 * The readable part of a project directory's name.
 *
 * Claude Code names each directory after the absolute path of the project,
 * with every `/` replaced by `-`, so `-Users-me-Projects-alpha` was
 * `/Users/me/Projects/alpha`. Because a directory name may itself contain a
 * `-`, that encoding cannot be reversed: `-Users-me-work-two-part-name` is
 * either `.../work/two-part-name` or `.../work/two/part/name` and nothing in
 * the string says which.
 *
 * So this drops only the part that is unambiguous and carries no information
 * — the `-Users-<someone>-` or `-home-<someone>-` home prefix, which is the
 * same for every row — and leaves the rest exactly as it is. Taking the last
 * `-`-separated segment would be shorter and would rename
 * `.../VSH/VSH-incident-speedlo` to "speedlo"; a label that misnames the
 * project is worse than a long one, and the full name is on the row's
 * tooltip either way.
 */
export function shortProject(name: string): string {
  const trimmed = name.replace(/^-(?:Users|home)-[^-]+-/, "");
  return trimmed === "" ? name : trimmed;
}

/**
 * What to print for a model id. Empty ids exist — a transcript line can omit
 * the model — and an empty cell would read as a rendering fault.
 */
export function modelLabel(name: string): string {
  return name === "" ? "(unnamed model)" : name;
}

/**
 * The models whose tokens are in the totals but whose cost is not, most
 * tokens first.
 *
 * This is what turns the backend's `unpricedTokens` into something the user
 * can act on: not just "the estimate is low" but which models made it low, so
 * a missing row in the price table is identifiable rather than mysterious.
 */
export function unpricedModels(byModel: Bucket[]): Bucket[] {
  return byModel
    .filter((bucket) => bucket.unpricedTokens > 0)
    .sort((a, b) => b.unpricedTokens - a.unpricedTokens);
}

/**
 * The span the estimate actually covers, or `null` when there is nothing to
 * describe.
 *
 * The headline figure otherwise states no period, and a total with no period
 * invites being read as a lifetime one. Its real window is however much
 * transcript history is on disk, which Claude Code prunes — so the honest
 * thing is to name the first and last day the scan actually saw. `byDay`
 * arrives newest-first, hence the ends being taken the way round they are.
 *
 * These are local calendar days: `analytics::summarize` buckets by the user's
 * own timezone rather than UTC, so the range means what it appears to mean.
 *
 * Deliberately not a count of days. `byDay.length` counts days *with usage*,
 * which is not the length of the span whenever a day was idle, and nothing
 * renders it — a field that has to be explained before it can be read is
 * better left out than shipped with a caveat.
 */
export function coveredRange(
  byDay: Bucket[],
): { first: string; last: string } | null {
  if (byDay.length === 0) return null;
  return { first: byDay[byDay.length - 1].name, last: byDay[0].name };
}

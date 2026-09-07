/**
 * How old the numbers in the popover are, and how the footer says so. Lifted
 * out of `Popover.svelte` so the branches are reachable from the test suite.
 */

/** Beyond this, the footer turns amber. */
export const STALE_WARNING_SECS = 5 * 60;

/**
 * Elapsed seconds since a snapshot was fetched, floored, never negative.
 *
 * The clamp is not decoration: `fetchedAt` is stamped by this app but `now`
 * ticks from a store, and a snapshot that arrives a few milliseconds "in the
 * future" of the last tick would otherwise render as a negative age.
 * Missing timestamp reads as zero — the footer says "just now" while the
 * first snapshot is still on its way, rather than claiming an age it does
 * not have.
 */
export function secondsSince(
  fetchedAt: string | null | undefined,
  now: Date,
): number {
  if (!fetchedAt) return 0;
  return Math.max(
    0,
    Math.floor((now.getTime() - new Date(fetchedAt).getTime()) / 1000),
  );
}

/**
 * Elapsed time, in words. Computed directly rather than through
 * `formatLong`: that one answers "how long until this timestamp", and for one
 * already in the past it returns "now" — rendering "updated now ago".
 */
export function formatAge(seconds: number): string {
  // "updated 0s ago" is what a refresh that just landed used to say. The
  // number is accurate and reads as broken, so the first ten seconds get
  // words instead.
  if (seconds < 10) return "updated just now";
  if (seconds < 60) return `updated ${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `updated ${minutes}m ago`;
  return `updated ${Math.floor(minutes / 60)}h ago`;
}

/** Whether an age has crossed into "this might not be current any more". */
export function ageIsStale(seconds: number): boolean {
  return seconds > STALE_WARNING_SECS;
}

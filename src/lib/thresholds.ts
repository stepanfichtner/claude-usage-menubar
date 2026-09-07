/**
 * What the settings window's "+ Add" button offers next. Lifted out of
 * `Settings.svelte` so the ceiling case is reachable from the test suite.
 */

/** Where the ladder starts when there is nothing to step up from. */
const SEED = 40;

/** The step between one threshold and the next the button proposes. */
const STEP = 10;

/** Notifications cannot fire above 100% used, so the ladder stops there. */
const CEILING = 100;

/**
 * One step up from the highest threshold, capped at the ceiling — or `null`
 * when that value is already in the list and there is nothing left to add.
 *
 * `null` happens exactly when 100 is already present. Below 90 the step
 * lands above every existing value and so cannot collide with one; at 90 or
 * above it is capped to 100, and 100 collides only with a 100 already there.
 * The old inline version returned that 100 regardless, so the list showed
 * two of them until `settings::sanitized()` deduplicated on save — the store
 * was never wrong, only the UI, and only until it reloaded.
 *
 * Stepping *down* to some free value instead was the alternative. It is
 * rejected because the number it would pick is arbitrary — with 50, 80, 90
 * and 100 set, nothing on screen explains why the button produced 70 rather
 * than 60 or 95 — and because every row is an editable number input, so
 * add-then-edit already reaches any value the ladder does not.
 */
export function nextThreshold(thresholds: number[]): number | null {
  const highest = thresholds.length ? Math.max(...thresholds) : SEED;
  const next = Math.min(CEILING, highest + STEP);
  return thresholds.includes(next) ? null : next;
}

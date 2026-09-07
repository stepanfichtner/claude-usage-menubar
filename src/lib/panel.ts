import type { Quota } from "./types";

/**
 * The popover's own logic, lifted out of `Popover.svelte` so it can be
 * exercised without rendering the component: how tall the OS window should
 * be, and how a snapshot's quotas are divided between the session meter and
 * the ring grid. The component keeps the parts that need a real window —
 * measuring `mainEl.scrollHeight` inside a `requestAnimationFrame`, and the
 * `setSize` call itself.
 */

/** The window's fixed width, matching `tauri.conf.json`'s popover window. */
export const PANEL_WIDTH = 320;

/**
 * A sane floor and ceiling, so a pathological snapshot (zero quotas, or fifty
 * of them) cannot produce an unusably short or absurdly tall window. Ordinary
 * quota counts (1-6ish) land well inside this range, and `overflow-y: auto`
 * on the panel's `main` is the fallback if a real one ever does not.
 */
export const MIN_HEIGHT = 140;
export const MAX_HEIGHT = 720;

/**
 * The height to resize the popover window to, or `null` when it should be
 * left alone.
 *
 * The `null` matters as much as the number. On a transparent window with
 * `backdrop-filter`, every `setSize` re-composites the backdrop even when the
 * new size equals the old one — calling it on every snapshot (most of which
 * do not change the content's height) made the panel visibly flash between
 * blurred and clear about once a poll.
 *
 * `Math.ceil` rather than round or floor: half a pixel short clips the last
 * row of content, half a pixel over is invisible.
 *
 * Note that no input can produce 0, because the floor is applied last —
 * which is what makes 0 usable as the caller's "nothing requested yet"
 * sentinel, so the first resize after launch is never mistaken for a repeat.
 */
export function nextPanelHeight(
  scrollHeight: number,
  lastHeight: number,
): number | null {
  const height = Math.min(
    MAX_HEIGHT,
    Math.max(MIN_HEIGHT, Math.ceil(scrollHeight)),
  );
  return height === lastHeight ? null : height;
}

/**
 * The session quota gets the full-width meter; everything else — any number
 * of weekly windows, present or future — goes in the ring grid.
 *
 * Deliberately a split by exclusion rather than a list of known ids: a new
 * model's weekly window must appear with no code change, which is the whole
 * point of consuming `limits[]` in the first place. Nothing is dropped, so
 * every quota the server sends is rendered somewhere.
 */
export function splitQuotas(quotas: Quota[]): {
  session: Quota | null;
  rings: Quota[];
} {
  return {
    session: quotas.find((q) => q.id === "session") ?? null,
    rings: quotas.filter((q) => q.id !== "session"),
  };
}

import { describe, expect, it } from "vitest";
import { MAX_HEIGHT, MIN_HEIGHT, nextPanelHeight, splitQuotas } from "./panel";
import type { Quota } from "./types";

describe("nextPanelHeight", () => {
  // The bounds exist so a pathological snapshot can't produce an unusable
  // window. Both sides of both bounds, because a clamp written the wrong way
  // round (`Math.max(MAX, Math.min(MIN, h))`) still returns a plausible
  // number for a mid-range measurement.
  it("clamps to the panel's floor and ceiling", () => {
    expect(nextPanelHeight(40, 0)).toBe(MIN_HEIGHT);
    expect(nextPanelHeight(MIN_HEIGHT, 0)).toBe(MIN_HEIGHT);
    expect(nextPanelHeight(MIN_HEIGHT + 1, 0)).toBe(MIN_HEIGHT + 1);
    expect(nextPanelHeight(MAX_HEIGHT - 1, 0)).toBe(MAX_HEIGHT - 1);
    expect(nextPanelHeight(MAX_HEIGHT, 0)).toBe(MAX_HEIGHT);
    expect(nextPanelHeight(5000, 0)).toBe(MAX_HEIGHT);
  });

  // `scrollHeight` is fractional on a scaled display. Rounding down by half a
  // pixel clips the last row of the footer; rounding up is invisible.
  it("rounds a fractional measurement up, not down", () => {
    expect(nextPanelHeight(300.2, 0)).toBe(301);
    expect(nextPanelHeight(300.8, 0)).toBe(301);
  });

  // The reason this function returns `number | null` at all: on a transparent
  // window with `backdrop-filter`, a `setSize` to the height it already has
  // still re-composites the backdrop, and the panel flashed between blurred
  // and clear about once a poll. Dropping the guard makes this return 300.
  it("asks for nothing when the height has not changed", () => {
    expect(nextPanelHeight(300, 300)).toBeNull();
    expect(nextPanelHeight(300.2, 301)).toBeNull();
  });

  // The caller starts `lastHeight` at 0 and relies on no measurement ever
  // producing 0, or the first resize after launch would be swallowed as a
  // repeat and the window would keep whatever size it opened at. That is only
  // true while the floor is applied last — a clamp that let a zero
  // measurement (content not laid out yet, `main` not yet bound) through
  // would break it.
  it("never answers with the caller's sentinel", () => {
    for (const measured of [0, 0.4, 1, 139.9]) {
      expect(nextPanelHeight(measured, 0)).toBe(MIN_HEIGHT);
    }
  });
});

function quota(id: string): Quota {
  return {
    id,
    label: `Label ${id}`,
    percent: 10,
    severity: "normal",
    resetsAt: null,
    isActive: false,
  };
}

describe("splitQuotas", () => {
  // The commitment this pins is "never hard-code which quotas exist": the
  // split is by exclusion, so a weekly window for a model nobody has heard of
  // yet renders with no code change. An implementation that listed the known
  // ring ids instead would pass every other assertion here and silently drop
  // this one — which is the exact failure the app consumes `limits[]` to
  // avoid.
  it("sends everything that is not the session quota to the ring grid", () => {
    const quotas = [
      quota("weekly_all"),
      quota("session"),
      quota("weekly:Fable"),
      quota("weekly:AModelThatDoesNotExistYet"),
    ];
    const { session, rings } = splitQuotas(quotas);

    expect(session?.id).toBe("session");
    expect(rings.map((q) => q.id)).toEqual([
      "weekly_all",
      "weekly:Fable",
      "weekly:AModelThatDoesNotExistYet",
    ]);
    expect(rings.length + 1).toBe(quotas.length);
  });

  // A snapshot without a session quota is not hypothetical — the server sends
  // `limits[]` and nothing guarantees its contents. The meter is skipped; the
  // rings are not.
  it("returns a null session rather than borrowing another quota", () => {
    const { session, rings } = splitQuotas([quota("weekly_all")]);
    expect(session).toBeNull();
    expect(rings.map((q) => q.id)).toEqual(["weekly_all"]);
  });

  it("survives an empty snapshot", () => {
    expect(splitQuotas([])).toEqual({ session: null, rings: [] });
  });
});

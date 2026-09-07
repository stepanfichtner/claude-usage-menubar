import { describe, expect, it } from "vitest";
import { nextThreshold } from "./thresholds";

describe("nextThreshold", () => {
  it("seeds an empty list at 50", () => {
    expect(nextThreshold([])).toBe(50);
  });

  // Each of these steps ten up from the *highest* value, not from the last
  // one — an implementation reading `thresholds.at(-1)` passes the first two
  // and fails the third.
  it.each([
    [[50], 60],
    [[50, 80], 90],
    [[90, 50], 100],
  ])("steps ten above the highest of %j", (thresholds, expected) => {
    expect(nextThreshold(thresholds)).toBe(expected);
  });

  it("caps the step at 100", () => {
    expect(nextThreshold([50, 80, 90])).toBe(100);
  });

  // The bug: `Math.min(100, highest + 10)` returned 100 again, so the UI
  // showed two 100% rows until `settings::sanitized()` deduplicated on save.
  it.each([[[100]], [[50, 80, 90, 100]], [[100, 50]]])(
    "offers nothing once 100 is already in %j",
    (thresholds) => {
      expect(nextThreshold(thresholds)).toBeNull();
    },
  );

  // `max="100"` on a number input is advisory, not enforced, so the stored
  // list can hold something above the ceiling. That is not the duplicate
  // case: 100 itself is still missing and still worth offering.
  it("still offers 100 when the highest value is above the ceiling", () => {
    expect(nextThreshold([150])).toBe(100);
    expect(nextThreshold([150, 100])).toBeNull();
  });
});

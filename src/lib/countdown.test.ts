import { describe, expect, it } from "vitest";
import { formatCompact, formatLong , formatResetTime } from "./countdown";

const now = new Date("2026-09-07T09:00:00Z");
const ahead = (minutes: number) =>
  new Date(now.getTime() + minutes * 60_000).toISOString();

describe("formatLong", () => {
  it("uses minutes under an hour", () => {
    expect(formatLong(ahead(45), now)).toBe("45m");
    expect(formatLong(ahead(1), now)).toBe("1m");
  });

  it("switches from minutes to hours at exactly sixty", () => {
    // Mirrors `compact_countdown_switches_from_minutes_to_hours_at_exactly_sixty`
    // in the Rust crate (src-tauri/src/tray.rs) — the two implementations must
    // agree at this boundary, not just at comfortable midpoints.
    expect(formatLong(ahead(59), now)).toBe("59m");
    expect(formatLong(ahead(60), now)).toBe("1h 0m");
  });

  it("uses hours and minutes under a day", () => {
    expect(formatLong(ahead(238), now)).toBe("3h 58m");
    expect(formatLong(ahead(60), now)).toBe("1h 0m");
  });

  it("switches from hours to days at exactly twenty-four hours", () => {
    // Mirrors `compact_countdown_uses_days_and_hours_beyond_that` in the Rust
    // crate — pinned there for `format_compact` at the exact minute-1440
    // transition, but never pinned here for `formatLong` until now.
    expect(formatLong(ahead(23 * 60 + 59), now)).toBe("23h 59m");
    expect(formatLong(ahead(24 * 60), now)).toBe("1d 0h");
  });

  it("uses days and hours beyond that", () => {
    expect(formatLong(ahead(2 * 24 * 60 + 13 * 60), now)).toBe("2d 13h");
    expect(formatLong(ahead(6 * 24 * 60), now)).toBe("6d 0h");
  });

  it("reads as now once the reset has passed", () => {
    expect(formatLong(ahead(-5), now)).toBe("now");
    expect(formatLong(ahead(0), now)).toBe("now");
  });
});

describe("formatCompact", () => {
  // Mirrors `tray::tests::compact_countdown_*` in the Rust crate
  // (src-tauri/src/tray.rs) — the two implementations must agree.
  it("uses minutes under an hour", () => {
    expect(formatCompact(ahead(47), now)).toBe("47m");
    expect(formatCompact(ahead(1), now)).toBe("1m");
  });

  it("switches from minutes to hours at exactly sixty", () => {
    expect(formatCompact(ahead(59), now)).toBe("59m");
    expect(formatCompact(ahead(60), now)).toBe("1h0m");
  });

  // The exact complaint that prompted this change: 1h49m was rounding down
  // to 2h in the menu bar, throwing away the minutes at the point they
  // matter most.
  it("keeps the smaller unit instead of rounding", () => {
    expect(formatCompact(ahead(109), now)).toBe("1h49m");
  });

  it("uses hours and minutes under a day", () => {
    expect(formatCompact(ahead(238), now)).toBe("3h58m");
    expect(formatCompact(ahead(23 * 60), now)).toBe("23h0m");
  });

  it("switches from hours to days at exactly twenty-four hours", () => {
    // Mirrors `compact_countdown_uses_days_and_hours_beyond_that` in the Rust
    // crate at the exact minute-1440 transition.
    expect(formatCompact(ahead(23 * 60 + 59), now)).toBe("23h59m");
    expect(formatCompact(ahead(24 * 60), now)).toBe("1d0h");
  });

  it("uses days and hours beyond that", () => {
    expect(formatCompact(ahead(2 * 24 * 60 + 13 * 60), now)).toBe("2d13h");
    expect(formatCompact(ahead(6 * 24 * 60), now)).toBe("6d0h");
  });

  it("reads as now once the reset has passed", () => {
    expect(formatCompact(ahead(-5), now)).toBe("now");
    expect(formatCompact(ahead(0), now)).toBe("now");
  });
});

describe("formatResetTime", () => {
  /** The weekday is ours, the clock is the machine's. Both halves asserted,
   *  because the fix is a split decision and either half alone would be wrong:
   *  forcing the locale outright would turn this user's 16:19 into 04:19 PM,
   *  and leaving it alone put a Czech "út" inside an otherwise English string.
   *
   *  What this does NOT catch: reverting `"en"` to `undefined`. Vitest runs
   *  under Node's default locale, which is English, so the reverted code
   *  produces the same output here and the assertion stays green. It bites on
   *  the format — the word order, the separator, a switched weekday style —
   *  not on the locale argument. Pinning that would need a child process with
   *  `LC_ALL` set, the way the analytics day-zone test does it for `TZ`, and
   *  for one word in one string that is more machinery than the risk earns.
   */
  it("names the weekday in English and leaves the time to the system", () => {
    const iso = "2026-09-08T16:19:00Z";
    const out = formatResetTime(iso);
    const systemTime = new Date(iso).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    });

    expect(out).toBe(`resets Tue ${systemTime}`);
  });

  /** Czech is reachable from this runner, so the assertion above is a statement
   *  about our choice rather than an accident of what ICU happens to ship. */
  it("could have said út, and does not", () => {
    const iso = "2026-09-08T16:19:00Z";
    expect(new Date(iso).toLocaleDateString("cs", { weekday: "short" })).toBe("út");
    expect(formatResetTime(iso)).not.toContain("út");
  });
});

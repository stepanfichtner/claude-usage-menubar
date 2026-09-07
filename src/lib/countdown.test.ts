import { describe, expect, it } from "vitest";
import { formatLong } from "./countdown";

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

  it("uses days and hours beyond that", () => {
    expect(formatLong(ahead(2 * 24 * 60 + 13 * 60), now)).toBe("2d 13h");
    expect(formatLong(ahead(6 * 24 * 60), now)).toBe("6d 0h");
  });

  it("reads as now once the reset has passed", () => {
    expect(formatLong(ahead(-5), now)).toBe("now");
    expect(formatLong(ahead(0), now)).toBe("now");
  });
});

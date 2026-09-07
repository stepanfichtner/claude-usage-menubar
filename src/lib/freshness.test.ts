import { describe, expect, it } from "vitest";
import { STALE_WARNING_SECS, ageIsStale, formatAge, secondsSince } from "./freshness";

const now = new Date("2026-09-07T09:00:00Z");
const secondsAgo = (seconds: number) =>
  new Date(now.getTime() - seconds * 1000).toISOString();

describe("secondsSince", () => {
  it("floors the elapsed seconds", () => {
    expect(secondsSince(secondsAgo(0), now)).toBe(0);
    expect(secondsSince(new Date(now.getTime() - 1999).toISOString(), now)).toBe(1);
    expect(secondsSince(secondsAgo(3661), now)).toBe(3661);
  });

  // `fetchedAt` is stamped by the app but `now` ticks from a store, so a
  // snapshot can arrive fractionally "ahead" of the last tick. Without the
  // clamp the footer renders a negative age; `formatAge` would then read
  // "updated just now" for it, which happens to be right, but the same
  // negative number is what `ageIsStale` is asked about.
  it("reads a future timestamp as zero rather than a negative age", () => {
    expect(secondsSince(new Date(now.getTime() + 5000).toISOString(), now)).toBe(0);
  });

  // Before the first snapshot lands there is no timestamp at all. Returning 0
  // is what makes the footer say "just now" instead of dating the panel to
  // 1970.
  it("reads a missing timestamp as zero", () => {
    expect(secondsSince(null, now)).toBe(0);
    expect(secondsSince(undefined, now)).toBe(0);
  });
});

describe("formatAge", () => {
  // The branch the 0.1.0 footer added and nothing held: "updated 0s ago" is
  // accurate and reads as broken, so the first ten seconds get words. Both
  // sides, because the whole point is where the words stop.
  it("says just now for the first ten seconds", () => {
    expect(formatAge(0)).toBe("updated just now");
    expect(formatAge(9)).toBe("updated just now");
    expect(formatAge(10)).toBe("updated 10s ago");
  });

  it("switches from seconds to minutes at exactly sixty", () => {
    expect(formatAge(59)).toBe("updated 59s ago");
    expect(formatAge(60)).toBe("updated 1m ago");
    expect(formatAge(119)).toBe("updated 1m ago");
  });

  it("switches from minutes to hours at exactly an hour", () => {
    expect(formatAge(59 * 60 + 59)).toBe("updated 59m ago");
    expect(formatAge(60 * 60)).toBe("updated 1h ago");
    expect(formatAge(2 * 60 * 60 + 59 * 60)).toBe("updated 2h ago");
  });
});

describe("ageIsStale", () => {
  // A strict `>`: five minutes on the nose is the last age that is still
  // fine, and the poll interval means the footer sits near this line
  // routinely, so an off-by-one here is an amber footer on a healthy app.
  it("turns amber only past the warning age", () => {
    expect(ageIsStale(STALE_WARNING_SECS - 1)).toBe(false);
    expect(ageIsStale(STALE_WARNING_SECS)).toBe(false);
    expect(ageIsStale(STALE_WARNING_SECS + 1)).toBe(true);
  });
});

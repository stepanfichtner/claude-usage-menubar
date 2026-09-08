import { describe, expect, it } from "vitest";
import {
  coveredRange,
  formatCost,
  formatTokens,
  modelLabel,
  shortProject,
  unpricedModels,
  type Bucket,
} from "./analytics";

function bucket(name: string, tokens: number, unpricedTokens = 0): Bucket {
  return { name, tokens, cost: 0, unpricedTokens };
}

describe("formatTokens", () => {
  it("shows millions to one decimal", () => {
    expect(formatTokens(125_400_000)).toBe("125.4M");
    expect(formatTokens(1_000_000)).toBe("1.0M");
  });

  // The reason this is not a one-line `(n / 1e6).toFixed(1)`: everything below
  // a million would render as "0.0M", so a lightly used machine would report
  // "0.0M tokens" for real usage and read as broken rather than as quiet.
  it("does not collapse everything under a million to 0.0M", () => {
    expect(formatTokens(12_500)).toBe("13K");
    expect(formatTokens(940)).toBe("940");
    expect(formatTokens(0)).toBe("0");
  });

  // The other end of the same mistake: a year of daily use passes a billion
  // tokens, and "1172.3M" is four digits of mantissa nobody reads at a glance.
  it("promotes a billion tokens to B rather than four-digit millions", () => {
    expect(formatTokens(1_172_321_200)).toBe("1.17B");
    expect(formatTokens(1_000_000_000)).toBe("1.00B");
  });

  // Each unit hands over at the point the smaller one would start printing a
  // four-digit mantissa, so no input can produce "1000K" or "1000.0M".
  it("hands over between units before the mantissa reaches four digits", () => {
    expect(formatTokens(999_999)).toBe("1.0M");
    expect(formatTokens(999_499)).toBe("999K");
    expect(formatTokens(999_950_000)).toBe("1.00B");
    expect(formatTokens(999_499_000)).toBe("999.5M");
  });
});

describe("formatCost", () => {
  it("is a plain amount when everything in the bucket was priced", () => {
    expect(formatCost(12.3456)).toBe("$12.35");
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(1234.5)).toBe("$1,234.50");
    expect(formatCost(9_876_543.21)).toBe("$9,876,543.21");
  });

  // The `+` is the whole point of the function. "$12.35" asserts what those
  // tokens cost; when some of them ran on a model with no price in the table
  // the only true statement is "at least $12.35", and dropping the marker
  // would present a number that is quietly, invisibly low as a total.
  it("marks a figure that excludes unpriced tokens as a floor", () => {
    expect(formatCost(12.3456, 1)).toBe("$12.35+");
    expect(formatCost(0, 4_000_000)).toBe("$0.00+");
  });

  it("treats zero unpriced tokens as nothing missing", () => {
    expect(formatCost(12.3456, 0)).toBe("$12.35");
  });

  // "$0.00" is the rendering reserved for a model that cost nothing, and a
  // priced bucket that spent a third of a cent did not cost nothing. Rounding
  // it down to "$0.00" makes real spend indistinguishable from free — the same
  // mistake as pricing an unknown model at zero, one row further along.
  it("does not round a real cost down to $0.00", () => {
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(0.0000001)).toBe("<$0.01");
    expect(formatCost(0.004, 500)).toBe("<$0.01+");
  });

  it("still says $0.00 when the cost really is zero", () => {
    expect(formatCost(0)).toBe("$0.00");
  });

  it("keeps rounding once a cost reaches a cent", () => {
    expect(formatCost(0.005)).toBe("$0.01");
    expect(formatCost(0.01)).toBe("$0.01");
  });
});

describe("coveredRange", () => {
  const day = (name: string): Bucket => ({
    name,
    tokens: 1,
    cost: 0,
    unpricedTokens: 0,
  });

  // The headline figure otherwise states no period at all, and its real window
  // is however much transcript history happens to be on disk — Claude Code
  // prunes old sessions, so it is neither "this month" nor a lifetime total.
  // byDay is newest-first, so the span runs from the last element to the first.
  it("spans the oldest and newest day with usage", () => {
    const range = coveredRange([
      day("2026-09-08"),
      day("2026-09-07"),
      day("2026-08-14"),
    ]);
    expect(range).toEqual({ first: "2026-08-14", last: "2026-09-08" });
  });

  it("handles a single day without inverting it", () => {
    expect(coveredRange([day("2026-09-08")])).toEqual({
      first: "2026-09-08",
      last: "2026-09-08",
    });
  });

  it("is null when there is nothing to describe", () => {
    expect(coveredRange([])).toBeNull();
  });
});

describe("shortProject", () => {
  it("drops the home prefix every row shares", () => {
    expect(shortProject("-Users-me-Projects-alpha")).toBe("Projects-alpha");
    expect(shortProject("-home-me-src-beta")).toBe("src-beta");
  });

  // The encoding replaces every "/" with "-", and a directory name may
  // contain a "-" of its own, so the split is not reversible. Taking the last
  // segment would render "/Users/me/VSH/VSH-incident-speedlo" as "speedlo" —
  // a label that names a different thing than the project it stands for.
  it("keeps a hyphenated project name whole", () => {
    expect(shortProject("-Users-me-VSH-VSH-incident-speedlo")).toBe(
      "VSH-VSH-incident-speedlo",
    );
    expect(shortProject("-Users-me-Projects-claude-usage-menubar")).toBe(
      "Projects-claude-usage-menubar",
    );
  });

  it("leaves a name it does not recognise alone rather than emptying it", () => {
    expect(shortProject("alpha")).toBe("alpha");
    expect(shortProject("-Users-me-")).toBe("-Users-me-");
    expect(shortProject("")).toBe("");
  });
});

describe("modelLabel", () => {
  it("names an empty model id instead of rendering a blank cell", () => {
    expect(modelLabel("")).toBe("(unnamed model)");
  });

  it("passes a real id through untouched", () => {
    expect(modelLabel("claude-opus-5")).toBe("claude-opus-5");
    expect(modelLabel("<synthetic>")).toBe("<synthetic>");
  });
});

describe("unpricedModels", () => {
  it("selects only the models whose tokens are missing from the estimate", () => {
    const models = [
      bucket("claude-opus-5", 5_000_000),
      bucket("some-future-model", 2_000_000, 2_000_000),
      bucket("<synthetic>", 100, 100),
    ];
    expect(unpricedModels(models).map((b) => b.name)).toEqual([
      "some-future-model",
      "<synthetic>",
    ]);
  });

  // Ordered by how much they cost the estimate, not by the cost-descending
  // order the buckets arrive in — every unpriced bucket has a cost of zero, so
  // their incoming order says nothing about which one matters.
  it("puts the biggest omission first", () => {
    const models = [
      bucket("small", 10, 10),
      bucket("large", 9_000_000, 9_000_000),
    ];
    expect(unpricedModels(models).map((b) => b.name)).toEqual([
      "large",
      "small",
    ]);
  });

  it("is empty when every model was priced", () => {
    expect(unpricedModels([bucket("claude-opus-5", 5_000_000)])).toEqual([]);
  });
});

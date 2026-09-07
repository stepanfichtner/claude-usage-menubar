import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { severityColor } from "./severity";
import type { Severity } from "./types";
// The stylesheet, read straight off disk. An `import appCss from
// "../app.css?raw"` looks tidier and is a trap: vitest stubs CSS modules out
// to an empty export, `?raw` included, so the assertion below would have
// searched an empty string and passed against anything (checked — it did).
// (`new URL("../app.css", import.meta.url)` does not work here either: under
// the jsdom environment the global `URL` is jsdom's, not Node's, and
// `readFileSync` rejects it.)
const readAppCss = () =>
  readFileSync(join(import.meta.dirname, "..", "app.css"), "utf8");

// Note on scope: the brief for this test asked for "every band boundary (50,
// 80, 90) and both sides of each". Those bands do not exist on this side of
// the wire. `severityColor` maps a `Severity`, not a percentage — the
// percentage-to-band mapping lives entirely in Rust (`Severity::from_percent`,
// pinned at all three boundaries by `usage::normalize::tests::
// derived_severity_boundaries`) and the frontend only ever receives the
// resulting string in the snapshot. There is no TypeScript code that turns a
// percentage into a severity, so there is nothing here for a band-boundary
// test to hold.

describe("severityColor", () => {
  it("gives each severity its own custom property", () => {
    expect(severityColor("normal")).toBe("var(--normal)");
    expect(severityColor("warning")).toBe("var(--warning)");
    expect(severityColor("high")).toBe("var(--high)");
    expect(severityColor("critical")).toBe("var(--critical)");
  });

  // The failure this catches is silent by construction: `var(--hgih)` is not
  // a syntax error anywhere, it simply resolves to nothing, and the ring
  // renders with no stroke at all. Nothing else in the build connects
  // severity.ts to app.css, so renaming a colour in one and not the other
  // ships a blank ring at exactly the moment the ring matters most.
  it("names only custom properties the stylesheet defines", () => {
    const appCss = readAppCss();
    const severities: Severity[] = ["normal", "warning", "high", "critical"];
    for (const severity of severities) {
      const property = severityColor(severity).replace(/^var\(|\)$/g, "");
      expect(appCss, `${severity} resolves to nothing`).toContain(`${property}:`);
    }
  });

  // The `default:` arm, and the only one a type-checked caller cannot reach.
  // Severities arrive over the IPC boundary as whatever Rust's `Severity`
  // serialises to, so adding a variant there — the same way `Severity::from_api`
  // already tolerates a server value it has never seen — delivers a string this
  // union does not contain. It has to render *a* colour rather than
  // `var(undefined)`.
  it("falls back to the normal colour for a severity it does not recognise", () => {
    expect(severityColor("chartreuse" as Severity)).toBe("var(--normal)");
  });
});

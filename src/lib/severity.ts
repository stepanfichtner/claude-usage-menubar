import type { Severity } from "./types";

export function severityColor(severity: Severity): string {
  switch (severity) {
    case "critical":
      return "var(--critical)";
    case "high":
      return "var(--high)";
    case "warning":
      return "var(--warning)";
    default:
      return "var(--normal)";
  }
}

export type Severity = "normal" | "warning" | "critical";

export interface Quota {
  id: string;
  label: string;
  percent: number;
  severity: Severity;
  /** RFC 3339 timestamp, or null when the server did not supply one. */
  resetsAt: string | null;
  isActive: boolean;
}

export interface UsageSnapshot {
  quotas: Quota[];
  fetchedAt: string;
  stale: boolean;
}

export interface Profile {
  displayName: string;
  planLabel: string;
}

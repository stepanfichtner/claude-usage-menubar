/** Mirrors `tray::format_long` in the Rust crate. Keep the two in step. */
export function formatLong(resetsAt: string, now: Date): string {
  const minutes = Math.floor(
    (new Date(resetsAt).getTime() - now.getTime()) / 60_000,
  );
  if (minutes <= 0) return "now";
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  const days = Math.floor(minutes / (24 * 60));
  const hours = Math.floor((minutes % (24 * 60)) / 60);
  return `${days}d ${hours}h`;
}

/**
 * Mirrors `tray::format_compact` in the Rust crate: the same two-unit case
 * table as `formatLong`, but with no space between them. Used wherever the
 * panel itself is tight on room — the ring cards — the same reason the menu
 * bar uses it.
 */
export function formatCompact(resetsAt: string, now: Date): string {
  const minutes = Math.floor(
    (new Date(resetsAt).getTime() - now.getTime()) / 60_000,
  );
  if (minutes <= 0) return "now";
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)}h${minutes % 60}m`;
  const days = Math.floor(minutes / (24 * 60));
  const hours = Math.floor((minutes % (24 * 60)) / 60);
  return `${days}d${hours}h`;
}

/** "resets Sat 10:29" — the absolute time beneath the countdown. */
export function formatResetTime(resetsAt: string): string {
  const date = new Date(resetsAt);
  // "en" rather than the system locale: every other word in this interface is
  // English, so a Czech or German weekday inside "resets … 16:19" reads as a
  // bug rather than as localisation. The *time* below stays on the system
  // locale deliberately — 24-hour vs AM/PM is a real preference the OS already
  // knows, and overriding it would be the actual regression.
  const weekday = date.toLocaleDateString("en", { weekday: "short" });
  const time = date.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  return `resets ${weekday} ${time}`;
}

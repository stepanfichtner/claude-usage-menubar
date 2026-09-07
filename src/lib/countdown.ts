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

/** "resets Sat 10:29" — the absolute time beneath the countdown. */
export function formatResetTime(resetsAt: string): string {
  const date = new Date(resetsAt);
  const weekday = date.toLocaleDateString(undefined, { weekday: "short" });
  const time = date.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  return `resets ${weekday} ${time}`;
}

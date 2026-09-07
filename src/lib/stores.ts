import { listen } from "@tauri-apps/api/event";
import { writable } from "svelte/store";
import type { Profile, UsageSnapshot } from "./types";

export interface SnapshotEvent {
  snapshot: UsageSnapshot;
  profile: Profile | null;
  signedOut: boolean;
}

export const snapshot = writable<SnapshotEvent | null>(null);

/** A clock the UI can tick from, so countdowns move without any network work. */
export const now = writable(new Date());
setInterval(() => now.set(new Date()), 1000);

listen<SnapshotEvent>("usage://snapshot", (event) => snapshot.set(event.payload));

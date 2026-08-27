// When to look for a new release, and when to say so. Kept out of the store
// because it is all pure decisions over a clock and three remembered values —
// which is the only part of the update flow that can be tested without cutting a
// real release.

import type { UIStatus, UpdateInfo } from "./types";

/** "off" disables the automatic check; anything else (including a missing key)
 *  leaves it on, so the default survives a storage that refuses to answer. */
const CHECK_KEY = "arvolo.updateCheck";
/** Epoch ms of the last check that actually reached the server. */
const LAST_KEY = "arvolo.updateLastCheck";
/** The version whose banner the user closed. */
const DISMISSED_KEY = "arvolo.updateDismissed";

/** Everything this module remembers, so a test can put it back to a clean slate.
 *  All three are global to the window: without this the cadence rules would leak
 *  between tests, and the first test to run a check would silence the next. */
export const STORAGE_KEYS = [CHECK_KEY, LAST_KEY, DISMISSED_KEY] as const;

/** One week.
 *
 *  Arvolo releases every few weeks, so hearing about one a few days late costs
 *  nobody anything — and every extra check is one more connection to github.com,
 *  which is the only third party this app talks to at all. */
export const CHECK_INTERVAL_MS = 7 * 24 * 60 * 60 * 1000;

/** How long after launch the first check may run.
 *
 *  Not immediately: the window is still applying the theme before its first
 *  frame, spawning the daemon, and retrying its way to a first snapshot. A
 *  banner that lands in the middle of that is noise on top of work. */
export const STARTUP_DELAY_MS = 45_000;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    // A private-mode webview can throw on any access. Every caller here has a
    // sane answer for "don't know", and none of them is worth failing to boot
    // over — same stance as the theme and the board order.
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not remembering is a worse experience, not a broken one: the check simply
    // runs again next launch.
  }
}

/** Whether the automatic check is allowed to run at all. On unless turned off. */
export function autoCheckEnabled(): boolean {
  return read(CHECK_KEY) !== "off";
}

export function setAutoCheckEnabled(on: boolean) {
  write(CHECK_KEY, on ? "on" : "off");
}

/** When the last successful check happened; 0 if never (or unreadable). */
export function lastCheck(): number {
  const raw = read(LAST_KEY);
  const n = raw === null ? NaN : Number(raw);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

export function markChecked(at: number) {
  write(LAST_KEY, String(at));
}

/** The version whose banner was closed, or "" for none. */
export function dismissedVersion(): string {
  return read(DISMISSED_KEY) ?? "";
}

export function dismissVersion(version: string) {
  write(DISMISSED_KEY, version);
}

/** Whether an *automatic* check should run right now.
 *
 *  This is the whole cadence, in one place: both triggers — the one a little
 *  after launch and the weekly timer — ask this, so eight launches in a morning
 *  still make one request. The manual button does not ask; it is explicit, and
 *  an explicit request that silently does nothing is worse than a slow one.
 */
export function dueForCheck(nowMs: number): boolean {
  if (!autoCheckEnabled()) return false;
  return nowMs - lastCheck() >= CHECK_INTERVAL_MS;
}

/** Statuses that mean the engine is in the middle of something.
 *
 *  `deposited` and `sharing` are deliberately absent: they are waiting on
 *  somebody else and survive a restart untouched. */
const BUSY: ReadonlySet<UIStatus> = new Set<UIStatus>([
  "active",
  "preparing",
  "cancelling",
]);

export function isBusy(statuses: readonly UIStatus[]): boolean {
  return statuses.some((s) => BUSY.has(s));
}

/** Whether to actually put a banner in front of someone.
 *
 *  Two brakes, both about not nagging. A version whose banner was closed does
 *  not come back — the next one will. And nothing is announced while a transfer
 *  is running: the check itself is just a GET and costs nobody anything, but
 *  proposing a restart in the middle of a send is the wrong thing at the worst
 *  moment. It comes back on its own when the board goes quiet. */
export function shouldAnnounce(
  info: UpdateInfo | null,
  dismissed: string,
  busy: boolean
): boolean {
  if (!info) return false;
  if (busy) return false;
  return info.version !== dismissed;
}

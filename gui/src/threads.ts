// Everything that has passed between you and one correspondent.
//
// This grouping was the phone's home screen for a while — one row per person,
// newest first. The home is the board again (see `HomeView`), and the grouping
// outlived it: a person's own screen shows their thread, and the sheet that
// draws it needs the same answer to the same question. It lives on its own
// because it is the one part of that screen that can be wrong in a way a
// screenshot does not show — two spellings of one person, or a row filed under
// nobody — which is also why it has a test of its own.

import { shortId } from "./format";
import type { UITransfer } from "./types";

/** Where a transfer goes while nobody has said who it is with. */
export const UNKNOWN = "\u0000unknown";

/** Everything that has passed between us and one correspondent. */
export interface Thread {
  /** Group key: the peer's id when there is one, else the label. */
  key: string;
  /** Who, as a person reads it. */
  who: string;
  /** Their public id, when the row has one. */
  peerId?: string;
  /** Newest first. */
  rows: UITransfer[];
}

/** Group transfers by who they are with, newest first inside and out.
 *
 * Exported for its test: the grouping *is* the screen, and it is the one part of
 * it that can be wrong in a way a screenshot would not show — two spellings of
 * the same person, or a row filed under nobody. */
export function threadsOf(rows: UITransfer[]): Thread[] {
  const by = new Map<string, Thread>();
  for (const r of rows) {
    // A share and a link have no correspondent by construction: nobody was sent
    // them, and they get their own row below rather than a thread.
    if (r.status === "sharing") continue;
    // Everything else without one is a transfer whose sender is not known *yet*
    // — a row built from events before the first snapshot carries an id, a name
    // and a size and nothing about who. Filing it under "unknown" keeps it on
    // screen until the snapshot says; dropping it made an accepted file vanish
    // from the list it was accepted in.
    const key = r.peerId || r.peer || UNKNOWN;
    const t: Thread = by.get(key) ?? {
      key,
      who: r.peer || (key === UNKNOWN ? "" : shortId(key)),
      peerId: r.peerId,
      rows: [],
    };
    t.rows.push(r);
    by.set(key, t);
  }
  const threads = [...by.values()];
  for (const t of threads) t.rows.sort((a, b) => b.firstSeen - a.firstSeen);
  // Arriving first, then the newest.
  //
  // Recency alone is the right order for a list of conversations, and the wrong
  // one for a list that mixes something waiting on you with something you have
  // already dealt with: a file being sent *to* you can need a decision, and a
  // decision does not belong under a record of what you sent this morning. So
  // direction is the first key and time is the second — which leaves the order
  // inside each half exactly as it was.
  threads.sort((a, b) => {
    const arriving = (t: Thread) => (t.rows[0]?.dir === "in" ? 0 : 1);
    return (
      arriving(a) - arriving(b) ||
      (b.rows[0]?.firstSeen ?? 0) - (a.rows[0]?.firstSeen ?? 0)
    );
  });
  return threads;
}

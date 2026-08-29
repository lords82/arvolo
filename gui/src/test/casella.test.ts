// The phone's home screen is a grouping, and a grouping is the one part of a
// screen that can be wrong in a way a screenshot does not show: a person split in
// two because one row knew their name and another only their key, or a row filed
// under nobody at all and quietly missing.

import { describe, expect, it } from "vitest";
import { threadsOf, UNKNOWN } from "../views/CasellaView";
import type { UITransfer } from "../types";

function t(over: Partial<UITransfer> = {}): UITransfer {
  return {
    key: "t1",
    id: 1,
    dir: "out",
    name: "file.txt",
    size: 1000,
    transferred: 0,
    status: "active",
    canPause: true,
    verified: false,
    swarmPeers: 0,
    downloadPeers: 0,
    firstSeen: 1_000,
    rank: 1_000,
    copiesServed: 0,
    bytesServed: 0,
    lastPickup: 0,
    fromDownload: 0,
    ...over,
  };
}

const GIULIA = "bsywxoy7hnch6oem45rf2deu5gc3f6zx";
const MARCO = "mk10dcqqqqqqqqqqqqqqqqqqqqqqqqqq";

describe("the Casella's grouping", () => {
  it("files everything with one person under one row, whatever the row calls them", () => {
    // The same key, seen twice: once while the address book had a name for it and
    // once before it did. Two rows in the list would read as two people.
    const threads = threadsOf([
      t({ key: "a", peerId: GIULIA, peer: "Giulia", firstSeen: 3_000 }),
      t({ key: "b", peerId: GIULIA, peer: undefined, firstSeen: 2_000 }),
    ]);
    expect(threads).toHaveLength(1);
    expect(threads[0].rows.map((r) => r.key)).toEqual(["a", "b"]);
    // And it is called by the name one of them knew, not by the key.
    expect(threads[0].who).toBe("Giulia");
  });

  it("puts what is arriving above what is going out, whatever the clock says", () => {
    // Recency alone is right for a list of conversations and wrong for one that
    // mixes a decision with a receipt: a file coming *to* you can need an
    // answer, and an answer does not belong under something you sent an hour
    // ago. Direction first, time second.
    const threads = threadsOf([
      t({ key: "sent-now", peerId: MARCO, peer: "Marco", dir: "out", firstSeen: 9_000 }),
      t({ key: "arrived-earlier", peerId: GIULIA, peer: "Giulia", dir: "in", firstSeen: 1_000 }),
    ]);
    expect(threads.map((x) => x.who)).toEqual(["Giulia", "Marco"]);
  });

  it("still puts the newest first inside each direction", () => {
    const threads = threadsOf([
      t({ key: "old-in", peerId: MARCO, peer: "Marco", dir: "in", firstSeen: 1_000 }),
      t({ key: "new-in", peerId: GIULIA, peer: "Giulia", dir: "in", firstSeen: 5_000 }),
    ]);
    expect(threads.map((x) => x.who)).toEqual(["Giulia", "Marco"]);
  });

  it("puts the newest exchange first, in the list and inside a row", () => {
    const threads = threadsOf([
      t({ key: "old", peerId: MARCO, peer: "Marco", firstSeen: 1_000 }),
      t({ key: "new", peerId: GIULIA, peer: "Giulia", firstSeen: 5_000 }),
      t({ key: "newest-marco", peerId: MARCO, peer: "Marco", firstSeen: 4_000 }),
    ]);
    expect(threads.map((x) => x.who)).toEqual(["Giulia", "Marco"]);
    expect(threads[1].rows[0].key).toBe("newest-marco");
  });

  it("leaves out a share, which is nobody's, and keeps a transfer that is somebody's", () => {
    // A served ticket and a public link were sent to nobody: they belong to the
    // row below the list, not to a thread.
    //
    // A transfer with no sender *yet* is the opposite case and the one that
    // matters: a row built from events alone — id, direction, name, size — knows
    // nothing about who until the next snapshot. Dropping it made a file vanish
    // from the list it had just been accepted in, which is the worst moment for
    // a file to be missing.
    const threads = threadsOf([
      t({ key: "share", peerId: undefined, peer: undefined, status: "sharing" }),
      t({ key: "just-accepted", peerId: undefined, peer: undefined, dir: "in" }),
      t({ key: "with-giulia", peerId: GIULIA, peer: "Giulia" }),
    ]);
    expect(threads.map((x) => x.key).sort()).toEqual([GIULIA, UNKNOWN].sort());
    expect(threads.find((x) => x.key === UNKNOWN)?.rows[0].key).toBe(
      "just-accepted"
    );
  });
});

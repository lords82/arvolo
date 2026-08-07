// Every store function the UI can reach, at least twice: the path that works and
// the path that goes wrong. The second one matters more — this app's bugs were all
// in the unhappy branch (an error swallowed into "nothing to show", a row dropped
// on a refusal, a click that reported success the daemon never gave).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());

import { useStore } from "../store";

let disposers: (() => void)[] = [];

async function boot(expectConnected = true) {
  useStore.setState({
    connected: false,
    status: null,
    guiVersion: "",
    loadError: null,
    contacts: [],
    contactsById: {},
    transfers: {},
    search: "",
    pauseAll: false,
    sheetPaths: null,
    incomingOfferId: null,
  });
  const dispose = await useStore.getState().init();
  disposers.push(dispose);
  if (expectConnected) {
    await vi.waitFor(() => expect(useStore.getState().connected).toBe(true));
  }
}

const s = () => useStore.getState();
const row = (key: string) => useStore.getState().transfers[key];

beforeEach(() => resetHarness());
afterEach(() => {
  disposers.forEach((d) => d());
  disposers = [];
});

describe("refreshContacts", () => {
  it("61. picks up a contact added elsewhere (CLI, another device)", async () => {
    await boot();
    expect(s().contacts).toHaveLength(0);
    harness.snapshot.contacts = [dto.contact({ name: "marta", id: "p9" })];
    await s().refreshContacts();
    expect(s().contacts).toHaveLength(1);
    expect(s().contactsById["p9"].name).toBe("marta");
  });

  it("62. keeps the book it has when the daemon cannot be asked", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    await boot();
    harness.fail = new Set(["listContacts"]);
    await s().refreshContacts();
    // Better a slightly stale name than a grid that empties itself on a hiccup.
    expect(s().contacts).toHaveLength(1);
  });

  it("63. a contacts_changed event refreshes the book on its own", async () => {
    await boot();
    harness.snapshot.contacts = [dto.contact({ name: "nuovo", id: "pX" })];
    harness.emit("contacts_changed");
    await vi.waitFor(() => expect(s().contactsById["pX"]?.name).toBe("nuovo"));
  });
});

describe("peerLabel / isVerified", () => {
  it("64. a saved contact wins over the sender's self-declared name", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "peer1" })];
    await boot();
    // The petname inside an offer is a claim, not an identity: never let it
    // override what the user themselves saved for that key.
    expect(s().peerLabel("peer1", "Mi Chiamo Proj")).toBe("proj");
  });

  it("65. an unknown peer falls back to its claimed name, then to a short id", async () => {
    await boot();
    expect(s().peerLabel("unknownkey", "Marta")).toBe("Marta");
    expect(s().peerLabel("if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha")).toContain("…");
    expect(s().peerLabel(null)).toBe("sconosciuto");
  });

  it("66. verified is only true for a contact actually marked verified", async () => {
    harness.snapshot.contacts = [
      dto.contact({ name: "a", id: "k1", verified: true }),
      dto.contact({ name: "b", id: "k2", verified: false }),
    ];
    await boot();
    expect(s().isVerified("k1")).toBe(true);
    expect(s().isVerified("k2")).toBe(false);
    expect(s().isVerified("stranger")).toBe(false);
    expect(s().isVerified(null)).toBe(false);
  });
});

describe("search", () => {
  it("67. setSearch stores the query verbatim", async () => {
    await boot();
    s().setSearch("relazione");
    expect(s().search).toBe("relazione");
  });

  it("68. clearing the search restores the empty query", async () => {
    await boot();
    s().setSearch("x");
    s().setSearch("");
    expect(s().search).toBe("");
  });
});

describe("navigation", () => {
  // Menu open/closed state used to live here as a single `openMenuKey`, so that
  // two rows could never have a menu open at once. It moved into the `Menu`
  // component, which owns its own state and closes on outside-click — the
  // exclusivity is now a property of there being one mounted at a time.

  it("69. arriving at a screen with no push event behind it refetches", async () => {
    await boot();
    const before = harness.recorder.listHistory;
    s().go("history");
    await vi.waitFor(() =>
      expect(harness.recorder.listHistory).toBe(before + 1)
    );
  });

  it("70. only one screen is ever current", async () => {
    await boot();
    s().go("people");
    expect(s().route).toBe("people");
    s().go("deposits");
    expect(s().route).toBe("deposits");
  });

  it("navigating away drops a stale error, so it cannot greet you on return", async () => {
    await boot();
    harness.fail = new Set(["listDeposits"]);
    s().go("deposits");
    await vi.waitFor(() => expect(s().depositsError).toBeTruthy());
    s().go("transfers");
    expect(s().depositsError).toBeNull();
  });
});

describe("send panel", () => {
  it("71. openSheet carries the dropped paths", async () => {
    await boot();
    s().openSheet(["/a.txt", "/b.txt"]);
    expect(s().sheetPaths).toEqual(["/a.txt", "/b.txt"]);
  });

  it("72. opening the panel dismisses the dialog underneath it", async () => {
    await boot();
    s().openIncoming("o1");
    s().openSheet(["/a.txt"]);
    // Two overlays at once is a UI that fights itself.
    expect(s().incomingOfferId).toBeNull();
  });

  it("73. closeSheet clears the paths so a stale file cannot be re-sent", async () => {
    await boot();
    s().openSheet(["/a.txt"]);
    s().closeSheet();
    expect(s().sheetPaths).toBeNull();
  });
});

describe("incoming modal", () => {
  it("74. openIncoming targets exactly one offer", async () => {
    await boot();
    s().openIncoming("o1");
    expect(s().incomingOfferId).toBe("o1");
  });

  it("75. closeIncoming dismisses it without deciding the offer", async () => {
    await boot();
    harness.emit({
      offer_received: {
        id: "o1",
        from: "p",
        name: "f",
        size: 1,
        note: "",
        sender_name: "",
      },
    });
    s().openIncoming("o1");
    s().closeIncoming();
    expect(s().incomingOfferId).toBeNull();
    // Dismissing the window is not rejecting the file.
    expect(row("oo1")).toBeTruthy();
    expect(harness.recorder.reject).toEqual([]);
  });
});

describe("ticket", () => {
  it("76. returns the arvc ticket the daemon minted", async () => {
    await boot();
    await expect(s().ticket(["/a.txt"])).resolves.toEqual({
      id: 1,
      ticket: "arvc-test",
    });
  });

  it("77. a refusal surfaces — the panel must not show a blank ticket", async () => {
    await boot();
    harness.fail = new Set(["serveTicket"]);
    await expect(s().ticket(["/a.txt"])).rejects.toThrow();
  });
});

describe("link", () => {
  it("78. returns the public URL, fragment key included", async () => {
    await boot();
    const url = await s().link("/a.txt", null, null);
    expect(url).toContain("#");
    expect(url).toContain("/dl/");
  });

  it("79. a refusal surfaces rather than yielding an unusable link", async () => {
    await boot();
    harness.fail = new Set(["createLink"]);
    await expect(s().link("/a.txt", null, null)).rejects.toThrow();
  });
});

describe("accept / reject", () => {
  it("80. accepting with no folder means the daemon's default", async () => {
    await boot();
    harness.emit({
      offer_received: { id: "o1", from: "p", name: "f", size: 1, note: "", sender_name: "" },
    });
    await s().accept("o1", null);
    expect(harness.recorder.accept).toEqual([["o1", null, null]]);
  });

  it("81. a refused accept keeps the offer — it is not silently lost", async () => {
    await boot();
    harness.emit({
      offer_received: { id: "o1", from: "p", name: "f", size: 1, note: "", sender_name: "" },
    });
    harness.fail = new Set(["acceptOffer"]);
    await expect(s().accept("o1", null)).rejects.toThrow();
    expect(row("oo1"), "the offer must still be there to retry").toBeTruthy();
  });

  it("82. a refused reject keeps the offer too", async () => {
    await boot();
    harness.emit({
      offer_received: { id: "o2", from: "p", name: "f", size: 1, note: "", sender_name: "" },
    });
    harness.fail = new Set(["rejectOffer"]);
    await expect(s().reject("o2")).rejects.toThrow();
    expect(row("oo2")).toBeTruthy();
  });
});

describe("pause / resume", () => {
  it("83. pause reaches the daemon", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
    await s().pause(1);
    expect(harness.recorder.pause).toEqual([1]);
  });

  it("84. a refused pause surfaces instead of pretending", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
    harness.fail = new Set(["pause"]);
    await expect(s().pause(1)).rejects.toThrow();
  });

  it("85. resume forwards the id and a refusal surfaces", async () => {
    await boot();
    harness.emit({ started: { id: 2, direction: "send", name: "f", total_size: 1 } });
    await s().resume(2);
    expect(harness.recorder.resume).toEqual([2]);

    harness.fail = new Set(["resume"]);
    await expect(s().resume(2)).rejects.toThrow();
  });
});

describe("markVerified", () => {
  it("86. a refused verify does not stamp a badge the daemon never granted", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "peer1", verified: false })];
    harness.snapshot.transfers = [dto.transfer({ id: 1, peer: "peer1" })];
    await boot();
    harness.fail = new Set(["markVerified"]);
    await expect(s().markVerified("proj")).rejects.toThrow();
    expect(row("t1").verified, "a claim of verified must come from the daemon").toBe(false);
  });

  it("87. verifying an unknown contact is refused, not invented", async () => {
    await boot();
    harness.fail = new Set(["markVerified"]);
    await expect(s().markVerified("nobody")).rejects.toThrow();
  });
});

describe("moveItem", () => {
  it("88. moving down swaps a row with the one below it", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "primo", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "send", name: "secondo", total_size: 1 } });
    const order = () =>
      Object.values(s().transfers)
        .sort((a, b) => b.rank - a.rank)
        .map((t) => t.name);
    const [top] = order();
    s().moveItem(top === "primo" ? "t1" : "t2", 1);
    expect(order()[0]).not.toBe(top);
  });

  it("89. a row never moves past the end of its own column", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "solo", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "recv", name: "altro", total_size: 1 } });
    const before = row("t1").rank;
    // Only sibling is in the other column, so there is nothing to swap with.
    s().moveItem("t1", 1);
    s().moveItem("t1", -1);
    expect(row("t1").rank).toBe(before);
  });

  it("90. moving an unknown row is a no-op, not a crash", async () => {
    await boot();
    expect(() => s().moveItem("t404", 1)).not.toThrow();
  });
});

describe("pause-all", () => {
  it("91. it is a toggle: the label state flips and flips back", async () => {
    await boot();
    expect(s().pauseAll).toBe(false);
    await s().togglePauseAll();
    expect(s().pauseAll).toBe(true);
    await s().togglePauseAll();
    expect(s().pauseAll).toBe(false);
  });

  it("92. with nothing running it asks the daemon for nothing", async () => {
    await boot();
    await s().togglePauseAll();
    expect(harness.recorder.pause).toEqual([]);
  });

  it("93. one refusal does not stop the others being paused", async () => {
    // A best-effort sweep: one stubborn transfer must not leave the rest running.
    await boot();
    for (const id of [1, 2, 3]) {
      harness.emit({ started: { id, direction: "send", name: `f${id}`, total_size: 1 } });
    }
    harness.fail = new Set(["pause"]);
    await s().togglePauseAll();
    expect(harness.recorder.pause).toEqual([1, 2, 3]);
    expect(s().pauseAll).toBe(true);
  });
});

describe("a failed action is never silent", () => {
  // The bug this closes: every action is fired from an `onClick` that cannot await
  // it, so a rejection vanished into the event loop. The button did nothing, said
  // nothing, and the user could only conclude the app was broken.
  it("96. a refused accept leaves a message the user can read", async () => {
    await boot();
    harness.emit({
      offer_received: { id: "o1", from: "p", name: "f", size: 1, note: "", sender_name: "" },
    });
    harness.fail = new Set(["acceptOffer"]);
    await expect(s().accept("o1", null)).rejects.toThrow();
    expect(s().actionError).toMatch(/accettare/i);
  });

  it.each([
    ["cancel", () => s().cancel(1), /annullare/i],
    ["pause", () => s().pause(1), /pausa/i],
    ["resume", () => s().resume(1), /riprendere/i],
    ["reject", () => s().reject("o1"), /rifiutare/i],
    ["markVerified", () => s().markVerified("proj"), /verificare/i],
    ["link", () => s().link("/a", null, null), /link/i],
    ["ticket", () => s().ticket(["/a"]), /ticket/i],
  ])("97. a refused %s reports itself", async (cmd, run, want) => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
    const apiName: Record<string, string> = {
      cancel: "cancel",
      pause: "pause",
      resume: "resume",
      reject: "rejectOffer",
      markVerified: "markVerified",
      link: "createLink",
      ticket: "serveTicket",
    };
    harness.fail = new Set([apiName[cmd]]);
    await expect(run()).rejects.toThrow();
    expect(s().actionError, `${cmd} must say why`).toMatch(want);
  });

  it("98. a later success clears the message", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
    harness.fail = new Set(["pause"]);
    await expect(s().pause(1)).rejects.toThrow();
    expect(s().actionError).toBeTruthy();

    harness.fail = new Set();
    await s().pause(1);
    expect(s().actionError).toBeNull();
  });

  it("99. the user can dismiss it", async () => {
    await boot();
    harness.fail = new Set(["createLink"]);
    await expect(s().link("/a", null, null)).rejects.toThrow();
    s().dismissActionError();
    expect(s().actionError).toBeNull();
  });
});

describe("history is grouped by when it happened, not when we noticed", () => {
  it("100. a transfer from days ago is dated by the engine, not by this session", async () => {
    // `firstSeen` used to be stamped when the GUI first saw a row, so every restart
    // refiled the whole history under "Oggi".
    const threeDaysAgo = Math.floor(Date.now() / 1000) - 3 * 86400;
    harness.snapshot.transfers = [dto.transfer({ id: 1, created: threeDaysAgo })];
    await boot();
    expect(row("t1").firstSeen).toBe(threeDaysAgo * 1000);
  });

  it("101. a daemon too old to date its transfers falls back to now", async () => {
    harness.snapshot.transfers = [dto.transfer({ id: 1, created: 0 })];
    const before = Date.now();
    await boot();
    expect(row("t1").firstSeen).toBeGreaterThanOrEqual(before);
  });
});

describe("clearFinished", () => {
  it("94. on an empty board it does nothing", async () => {
    await boot();
    await expect(s().clearFinished()).resolves.toBeUndefined();
    expect(Object.keys(s().transfers)).toHaveLength(0);
  });

  it("95. it clears the concluded and closes the menu, keeping arrivals to decide", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "done", total_size: 1 } });
    harness.emit({ completed: { id: 1, path: null } });
    harness.emit({
      offer_received: { id: "o1", from: "p", name: "in", size: 1, note: "", sender_name: "" },
    });
    await s().clearFinished();

    expect(row("t1")).toBeUndefined();
    expect(row("oo1"), "an undecided arrival is not history").toBeTruthy();
  });
});

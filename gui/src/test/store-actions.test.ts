// Every store function the UI can reach, at least twice: the path that works and
// the path that goes wrong. The second one matters more — this app's bugs were all
// in the unhappy branch (an error swallowed into "nothing to show", a row dropped
// on a refusal, a click that reported success the daemon never gave).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { P2P_TICKET, PEER_ID, dto, harness, makeIpcMock, resetHarness, pick } from "./mocks";

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
    sheetPicks: null,
    incomingOfferId: null,
    // Where the app opens. Left out, every test inherited whatever screen the
    // one before it had navigated to — which is fine until a test pushes, and
    // then the next one starts one screen deep with no way to know it.
    route: "transfers",
    stack: ["transfers"],
    threadKey: null,
    personOpen: null,
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
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: PEER_ID })];
    await boot();
    // The petname inside an offer is a claim, not an identity: never let it
    // override what the user themselves saved for that key.
    expect(s().peerLabel(PEER_ID, "Mi Chiamo Proj")).toBe("proj");
  });

  it("65. an unknown peer falls back to its claimed name, then to a short id", async () => {
    await boot();
    expect(s().peerLabel("unknownkey", "Marta")).toBe("Marta");
    expect(s().peerLabel(PEER_ID)).toContain("…");
    expect(s().peerLabel(null)).toBe("unknown");
  });

  it("65a. our own id is our own devices, not an unknown peer", async () => {
    await boot();
    // A send to yourself names the identity every one of your machines shares,
    // so the id comes back as our own — and a shortened id in that row would
    // read as a stranger nobody saved.
    const mine = s().status?.public_id ?? "";
    expect(mine).not.toBe("");
    expect(s().peerLabel(mine)).toBe("Your devices");
    // Even against a name the sender claimed for itself, which is us.
    expect(s().peerLabel(mine, "Marta")).toBe("Your devices");
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
  it("71. openSheet carries the registered picks", async () => {
    await boot();
    s().openSheet([pick("a.txt"), pick("b.txt")]);
    expect(s().sheetPicks).toEqual([pick("a.txt"), pick("b.txt")]);
  });

  it("72. opening the panel dismisses the dialog underneath it", async () => {
    await boot();
    s().openIncoming("o1");
    s().openSheet([pick("a.txt")]);
    // Two overlays at once is a UI that fights itself.
    expect(s().incomingOfferId).toBeNull();
  });

  it("73. closeSheet clears the paths so a stale file cannot be re-sent", async () => {
    await boot();
    s().openSheet([pick("a.txt")]);
    s().closeSheet();
    expect(s().sheetPicks).toBeNull();
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
      ticket: P2P_TICKET,
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
    expect(s().actionRefusal, "a plain failure carries no code").toBeNull();
  });

  it("79b. a file the relay will not hold keeps its code and the relay's limit", async () => {
    await boot();
    harness.refuse = { createLink: { code: "tooLarge", limitBytes: 5 * 1024 ** 3 } };
    await expect(s().link("/big.iso", null, null)).rejects.toThrow();
    expect(s().actionRefusal).toEqual({ code: "tooLarge", limitBytes: 5 * 1024 ** 3 });
    // Dismissing the sentence dismisses its code with it: a stale code against
    // the next, unrelated failure would translate it as the wrong thing.
    s().dismissActionError();
    expect(s().actionRefusal).toBeNull();
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
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: PEER_ID, verified: false })];
    harness.snapshot.transfers = [dto.transfer({ id: 1, peer: PEER_ID })];
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

describe("reorderItems", () => {
  const order = () =>
    Object.values(s().transfers)
      .sort((a, b) => b.rank - a.rank)
      .map((t) => t.key);

  it("88. the rows end up in the order they were given", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "primo", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "send", name: "secondo", total_size: 1 } });
    const before = order();
    s().reorderItems([...before].reverse());
    expect(order()).toEqual([...before].reverse());
  });

  it("89. rows left out of the list keep their place", async () => {
    await boot();
    for (const id of [1, 2, 3]) {
      harness.emit({
        started: { id, direction: "send", name: `f${id}`, total_size: 1 },
      });
    }
    const [top, mid, bottom] = order();
    const untouched = row(mid).rank;
    // A section is reordered on its own, so only the ranks of the rows named
    // may move — the one between them must not be dragged along.
    s().reorderItems([bottom, top]);
    expect(row(mid).rank).toBe(untouched);
    expect(order()).toEqual([bottom, mid, top]);
  });

  it("90. unknown keys are ignored rather than crashing", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "solo", total_size: 1 } });
    const before = row("t1").rank;
    expect(() => s().reorderItems(["t404", "t1"])).not.toThrow();
    expect(() => s().reorderItems([])).not.toThrow();
    expect(row("t1").rank).toBe(before);
  });

  it("161. a reorder is written down, so the next run can have it back", async () => {
    localStorage.removeItem("arvolo.order");
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "primo", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "send", name: "secondo", total_size: 1 } });
    // Nothing is written until the user actually says something about the order.
    expect(localStorage.getItem("arvolo.order")).toBeNull();

    const keys = Object.values(s().transfers)
      .sort((a, b) => b.rank - a.rank)
      .map((t) => t.key);
    s().reorderItems([...keys].reverse());

    const saved = JSON.parse(localStorage.getItem("arvolo.order")!);
    expect(saved[keys[1]]).toBeGreaterThan(saved[keys[0]]);
  });

  it("162. a fresh boot picks the remembered order back up", async () => {
    // The rank map is read once, when the module loads — so a *new run* of the
    // app is a new module, not a new store state.
    localStorage.setItem("arvolo.order", JSON.stringify({ t1: 40, t2: 10 }));
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "primo" }),
      dto.transfer({ id: 2, name: "secondo" }),
      // Never placed by hand: it has no remembered rank, and belongs on top.
      dto.transfer({ id: 3, name: "arrivato dopo" }),
    ];
    vi.resetModules();
    const { useStore: fresh } = await import("../store");
    const dispose = await fresh.getState().init();
    disposers.push(dispose);
    await vi.waitFor(() => expect(fresh.getState().connected).toBe(true));

    const order = Object.values(fresh.getState().transfers)
      .sort((a, b) => b.rank - a.rank)
      .map((t) => t.name);
    expect(order).toEqual(["arrivato dopo", "primo", "secondo"]);
    localStorage.removeItem("arvolo.order");
  });

  it("163. a webview that refuses localStorage still reorders", async () => {
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("nope");
      });
    try {
      await boot();
      harness.emit({ started: { id: 1, direction: "send", name: "primo", total_size: 1 } });
      harness.emit({ started: { id: 2, direction: "send", name: "secondo", total_size: 1 } });
      const keys = Object.values(s().transfers)
        .sort((a, b) => b.rank - a.rank)
        .map((t) => t.key);
      expect(() => s().reorderItems([...keys].reverse())).not.toThrow();
      // The order still moved; only the remembering was lost.
      expect(
        Object.values(s().transfers)
          .sort((a, b) => b.rank - a.rank)
          .map((t) => t.key)
      ).toEqual([...keys].reverse());
    } finally {
      setItem.mockRestore();
    }
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

// A send spends its first minutes reading and encrypting the payload, with
// nothing on the wire and the recipient not yet told anything. Shown as "active"
// it was indistinguishable from a transfer stuck at zero bytes.
describe("a send that is still preparing", () => {
  it("says so, instead of reading as an active transfer at 0 B", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "big.ipsw", total_size: 11e9 } });
    harness.emit({ preparing: { id: 1 } });
    expect(row("t1").status).toBe("preparing");
    // And it stays on the board rather than dropping out of every section.
    expect(row("t1").transferred).toBe(0);
  });

  it("becomes an ordinary transfer once bytes actually move", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "big.ipsw", total_size: 11e9 } });
    harness.emit({ preparing: { id: 1 } });
    harness.emit({ progress: { id: 1, transferred: 1024, total_size: 11e9 } });
    expect(row("t1").status).toBe("active");
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
    expect(s().actionError).toMatch(/accept the file/i);
  });

  it.each([
    ["cancel", () => s().cancel(1), /cancel it/i],
    ["pause", () => s().pause(1), /pause it/i],
    ["resume", () => s().resume(1), /resume it/i],
    ["reject", () => s().reject("o1"), /reject the file/i],
    ["markVerified", () => s().markVerified("proj"), /verify/i],
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

  // The report this closes: a send the recipient had already dropped was
  // cancelled, and the row sat on "Annullamento…" for good. The daemon answered
  // "ok" to an id its engine no longer had, so the `cancelled` event that would
  // have ended the row was never coming.
  it("97a. a cancel the daemon refuses puts the row back, not on 'cancelling'", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
    harness.fail = new Set(["cancel"]);
    await expect(s().cancel(1)).rejects.toThrow();
    expect(row("t1").status).toBe("active");
  });

  // And when the daemon accepts but the event never lands, the board asks rather
  // than believing its own optimism for ever.
  it("97b. a cancel with no event settles from the snapshot", async () => {
    vi.useFakeTimers();
    try {
      await boot();
      harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
      // The daemon takes it and says nothing more — but its list already has the
      // row as cancelled, which is what the re-read finds.
      harness.snapshot.transfers = [
        dto.transfer({ id: 1, name: "f", status: "cancelled" }),
      ];
      await s().cancel(1);
      expect(row("t1").status).toBe("cancelling");
      await vi.advanceTimersByTimeAsync(20_000);
      expect(row("t1").status).toBe("cancelled");
    } finally {
      vi.useRealTimers();
    }
  });

  // The case actually caught in the wild: the daemon was gone by the time the
  // cancel was asked for, so there was no event *and* no snapshot to fall back
  // on. A row must not keep claiming to be cancelling on nobody's authority.
  it("97c. a cancel with nothing answering gives the row back", async () => {
    vi.useFakeTimers();
    try {
      await boot();
      harness.emit({ started: { id: 1, direction: "send", name: "f", total_size: 1 } });
      await s().cancel(1);
      expect(row("t1").status).toBe("cancelling");
      // Nothing answers any more — not the cancel's event, not the re-read.
      harness.fail = new Set(["listTransfers", "status"]);
      await vi.advanceTimersByTimeAsync(20_000);
      expect(row("t1").status).toBe("active");
    } finally {
      vi.useRealTimers();
    }
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
    // refiled the whole history under "Today".
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

describe("the way back, on a phone", () => {
  // Android's back is the system's, and it arrives here as a history pop — so
  // every screen has to have left an entry behind, and one press has to undo
  // exactly one screen. A thread that did neither let the back gesture walk
  // straight past an open person and pop the list underneath them, which from
  // the first screen means leaving the app.
  it("102. takes an open thread away before it touches the stack", async () => {
    await boot();
    s().push("settings");
    s().openThread("p1");
    expect(s().threadKey).toBe("p1");

    s().back();
    expect(s().threadKey, "the thread goes first").toBeNull();
    expect(s().route, "and the screen under it is still there").toBe("settings");

    s().back();
    expect(s().route).toBe("transfers");
    expect(s().stack).toHaveLength(1);
  });

  // The entry the system's back pops is left by whatever is open, not by the
  // store — see `backstack`, and the tests for it in backstack.test.ts.

  it("106. a transfer the engine took by itself asks who it is from", async () => {
    // Accepting by hand carries the person over from the offer. Nothing does
    // that for a file the engine takes on its own — a trusted contact's, or one
    // of your own machines' — and the event that announces it says only id,
    // name and size. The row sat there as "Somebody", with a question mark for
    // a face, until something else happened to reload.
    harness.snapshot.contacts = [dto.contact({ name: "giulia", id: PEER_ID })];
    await boot();
    // What the daemon would say if asked: this transfer is from Giulia.
    harness.snapshot.transfers = [
      dto.transfer({ id: 77, direction: "recv", peer: PEER_ID }),
    ];
    // What the event says: id, name, size. Nothing about who.
    harness.emit({
      started: { id: 77, direction: "recv", name: "da-me.bin", total_size: 10 },
    });
    expect(row("t77").peerId, "the event carried nobody").toBeUndefined();
    await vi.waitFor(() => expect(row("t77").peer).toBe("giulia"), {
      timeout: 3000,
    });
  });

  it("105. pushing to a screen fetches what that screen shows", async () => {
    // A phone reaches every inner screen by pushing and never by `go`, so for
    // as long as only `go` fetched, those screens opened and stayed empty:
    // "Your devices" said "loading…" for as long as anybody cared to look.
    await boot();
    const before = harness.recorder.listHistory;
    s().push("history");
    await vi.waitFor(() =>
      expect(harness.recorder.listHistory).toBe(before + 1)
    );
  });

  it("104. a person's page goes before a thread, which goes before the stack", async () => {
    // The palette can open a person over anything — including over an open
    // thread. Back then has three things above the root, and takes them
    // topmost first.
    await boot();
    s().push("settings");
    s().openThread("p1");
    s().openPerson("Giulia");

    s().back();
    expect(s().personOpen).toBeNull();
    expect(s().threadKey, "the thread is still under it").toBe("p1");
    s().back();
    expect(s().threadKey).toBeNull();
    expect(s().route).toBe("settings");
    s().back();
    expect(s().route).toBe("transfers");
  });
});

// The board's brain: a daemon snapshot seeds it, pushed engine events mutate it.
// Every regression we hit in this GUI showed up here first — a board that stayed
// empty, a row that vanished while the daemon still had the transfer, a cancel
// that moved no number. These lock that behaviour down.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());

import { useStore } from "../store";

/** Disposers for the subscriptions each `boot()` opens. Left running, their retry
 *  loop would keep re-seeding a *later* test's store from a stale snapshot. */
let disposers: (() => void)[] = [];

/** Fresh store + a booted subscription, as `App` does on mount. Waits for the first
 *  snapshot to land: `init()` kicks the seed off but does not await it (a slow
 *  daemon must not block the window from opening). */
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
  return dispose;
}

const rows = () => Object.values(useStore.getState().transfers);
const row = (key: string) => useStore.getState().transfers[key];

beforeEach(() => {
  resetHarness();
});

afterEach(() => {
  disposers.forEach((d) => d());
  disposers = [];
});

describe("snapshot seeding", () => {
  it("1. seeds transfers, offers and contacts from the daemon", async () => {
    harness.snapshot.transfers = [dto.transfer({ id: 3, name: "a.txt" })];
    harness.snapshot.pending = [dto.offer({ id: "o9" })];
    harness.snapshot.contacts = [dto.contact()];
    await boot();

    expect(useStore.getState().connected).toBe(true);
    expect(row("t3").name).toBe("a.txt");
    expect(row("oo9").status).toBe("incoming");
    expect(useStore.getState().contacts).toHaveLength(1);
  });

  it("2. self-heals: a snapshot that fails first is retried until it lands", async () => {
    // The pump only emits `connected` on a *change*, so a webview that loads while
    // it is already subscribed gets no event. If the first snapshot also failed, the
    // board used to sit empty forever with nothing left to wake it.
    harness.fail = new Set(["status", "listTransfers"]);
    await boot(false);
    expect(useStore.getState().connected).toBe(false);
    expect(rows()).toHaveLength(0);

    harness.fail = new Set();
    harness.snapshot.transfers = [dto.transfer({ id: 1 })];
    await vi.waitFor(
      () => {
        expect(useStore.getState().connected).toBe(true);
        expect(rows()).toHaveLength(1);
      },
      { timeout: 5000, interval: 100 }
    );
  });

  it("3. a reconnect re-seeds the board", async () => {
    await boot();
    expect(rows()).toHaveLength(0);
    harness.snapshot.transfers = [dto.transfer({ id: 5, name: "late.bin" })];
    harness.setConnected(true);
    await vi.waitFor(() => expect(row("t5")?.name).toBe("late.bin"));
  });
});

describe("the board never lies about what the daemon holds", () => {
  it("49. a daemon holding sends is never shown as an empty board", async () => {
    // The failure this pins down: the board read "Connesso · 0 in totale" while the
    // daemon held two live sends. A snapshot that cannot be read must never be
    // rendered as "there is nothing" — that is indistinguishable from the truth.
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "Constatazione.pdf", status: "deposited" }),
      dto.transfer({ id: 2, name: "Constatazione.pdf", status: "active" }),
    ];
    await boot();
    expect(rows()).toHaveLength(2);
    expect(useStore.getState().loadError).toBeNull();
  });

  it("50. a snapshot that fails says so — it does not report an empty daemon", async () => {
    harness.fail = new Set(["listTransfers"]);
    await boot(false);
    expect(useStore.getState().loadError).toMatch(/can't read the transfers/i);
    expect(useStore.getState().connected).toBe(false);
  });

  it("51. a failed refresh keeps the rows it already had, it does not blank them", async () => {
    harness.snapshot.transfers = [dto.transfer({ id: 1, name: "keep.me" })];
    await boot();
    expect(rows()).toHaveLength(1);

    harness.fail = new Set(["listTransfers"]);
    await useStore.getState().reload();

    expect(row("t1"), "the row must survive a failed refresh").toBeTruthy();
    expect(useStore.getState().loadError).toBeTruthy();
  });

  it("52. a recovered snapshot clears the error", async () => {
    harness.fail = new Set(["status"]);
    await boot(false);
    expect(useStore.getState().loadError).toBeTruthy();

    harness.fail = new Set();
    harness.snapshot.transfers = [dto.transfer({ id: 1 })];
    await useStore.getState().reload();

    expect(useStore.getState().loadError).toBeNull();
    expect(useStore.getState().connected).toBe(true);
    expect(rows()).toHaveLength(1);
  });

  it("53. every command the UI can issue is wired to a real bridge call", async () => {
    // A command the bridge does not expose fails at runtime only — and the store
    // used to swallow exactly that. Assert the surface the UI depends on exists.
    const { api } = (await import("../ipc")) as any;
    for (const cmd of [
      "status",
      "listTransfers",
      "listPending",
      "listContacts",
      "sendTo",
      "serveTicket",
      "createLink",
      "acceptOffer",
      "rejectOffer",
      "pause",
      "resume",
      "cancel",
      "remove",
      "markVerified",
      "guiVersion",
    ]) {
      expect(typeof api[cmd], `api.${cmd} must exist`).toBe("function");
    }
  });

  it("54. a send lands on the board even if no snapshot ever arrives", async () => {
    // Belt and braces: the row must come from the pushed `started` event alone, so a
    // send is visible even when the snapshot path is broken.
    harness.fail = new Set(["listTransfers", "status"]);
    await boot(false);
    await useStore.getState().send("proj", ["/a.pdf"], "");
    harness.emit({ started: { id: 1,
      direction: "send",
      name: "a.pdf",
      total_size: 10 } });
    expect(row("t1").name).toBe("a.pdf");
  });
});

describe("engine events → rows", () => {
  it("4. `started` creates the row", async () => {
    await boot();
    harness.emit({ started: { id: 1,
      direction: "send",
      name: "x.bin",
      total_size: 200 } });
    expect(row("t1")).toMatchObject({
      dir: "out",
      name: "x.bin",
      size: 200,
      status: "active",
    });
  });

  it("5. `progress` advances bytes without losing the name", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "x.bin", total_size: 200 } });
    harness.emit({ progress: { id: 1, transferred: 50, total_size: 200 } });
    expect(row("t1")).toMatchObject({ name: "x.bin", transferred: 50 });
  });

  it("6. `completed` marks it done and keeps the saved path", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "recv", name: "in.bin", total_size: 80 } });
    harness.emit({ completed: { id: 1, path: "/Users/ls/Arvolo/in.bin" } });
    expect(row("t1")).toMatchObject({
      status: "completed",
      transferred: 80,
      path: "/Users/ls/Arvolo/in.bin",
    });
  });

  it("7. `deposited` — awaiting pickup, not delivered", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "m.bin", total_size: 10 } });
    harness.emit({ deposited: { id: 1 } });
    expect(row("t1").status).toBe("deposited");
  });

  it("8. `waiting` carries the reason the daemon gave", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "m.bin", total_size: 10 } });
    harness.emit({ waiting: { id: 1, reason: "relay unavailable" } });
    expect(row("t1")).toMatchObject({ status: "stalled", reason: "relay unavailable" });
  });

  it("9. `paused` and `failed` and `cancelled` land on their statuses", async () => {
    await boot();
    for (const id of [1, 2, 3]) {
      harness.emit({ started: { id, direction: "send", name: `f${id}`, total_size: 10 } });
    }
    harness.emit({ paused: { id: 1, reason: "by user" } });
    harness.emit({ failed: { id: 2, error: "boom" } });
    harness.emit({ cancelled: { id: 3 } });
    expect(row("t1")).toMatchObject({ status: "paused", reason: "by user" });
    expect(row("t2")).toMatchObject({ status: "failed", reason: "boom" });
    expect(row("t3").status).toBe("cancelled");
  });

  it("10. `offer_received` shows the sender's note and name", async () => {
    await boot();
    harness.emit({ offer_received: { id: "o1",
      from: "peer9",
      name: "foto.zip",
      size: 900,
      note: "le foto di ieri",
      sender_name: "Marta" } });
    expect(row("oo1")).toMatchObject({
      status: "incoming",
      dir: "in",
      note: "le foto di ieri",
      senderName: "Marta",
      offerId: "o1",
    });
  });

  it("11b. a re-offer of the same file replaces the old row, id and all", async () => {
    // A sender still retrying re-posts its offer and retracts the previous one, so
    // the id changes. Keeping both left a row whose Accetta pointed at an id the
    // daemon had already dropped — the button then did nothing, silently.
    await boot();
    const offer = (id: string) => ({
      offer_received: {
        id,
        from: "peer1",
        name: "relazione.pdf",
        size: 10,
        note: "",
        sender_name: "",
      },
    });
    harness.emit(offer("old1"));
    harness.emit(offer("new2"));

    expect(rows().filter((t) => t.status === "incoming")).toHaveLength(1);
    expect(row("oold1"), "the dead id must not linger").toBeUndefined();
    expect(row("onew2").offerId).toBe("new2");
  });

  it("11c. a different file from the same sender is not superseded", async () => {
    await boot();
    const offer = (id: string, name: string) => ({
      offer_received: { id, from: "peer1", name, size: 10, note: "", sender_name: "" },
    });
    harness.emit(offer("a", "uno.txt"));
    harness.emit(offer("b", "due.txt"));
    expect(rows().filter((t) => t.status === "incoming")).toHaveLength(2);
  });

  it("11d. the same file from a different sender is its own offer", async () => {
    await boot();
    harness.emit({
      offer_received: { id: "a", from: "peer1", name: "x.txt", size: 1, note: "", sender_name: "" },
    });
    harness.emit({
      offer_received: { id: "b", from: "peer2", name: "x.txt", size: 1, note: "", sender_name: "" },
    });
    expect(rows().filter((t) => t.status === "incoming")).toHaveLength(2);
  });

  it("11. an event for an unknown id still produces a row (no lost sends)", async () => {
    // A drop→send can land before any snapshot; the row must appear anyway.
    await boot();
    harness.emit({ progress: { id: 42, transferred: 5, total_size: 10 } });
    expect(row("t42")).toBeTruthy();
  });
});

describe("actions reach the daemon", () => {
  it("12. pause/resume forward the id", async () => {
    await boot();
    harness.emit({ started: { id: 4, direction: "send", name: "p", total_size: 1 } });
    await useStore.getState().pause(4);
    await useStore.getState().resume(4);
    expect(harness.recorder.pause).toEqual([4]);
    expect(harness.recorder.resume).toEqual([4]);
  });

  it("13. accept sends the chosen folder and drops the offer row", async () => {
    await boot();
    harness.emit({ offer_received: { id: "o1",
      from: "p",
      name: "f",
      size: 1,
      note: "",
      sender_name: "" } });
    await useStore.getState().accept("o1", "/tmp/dest");
    expect(harness.recorder.accept).toEqual([["o1", "/tmp/dest", null]]);
    expect(row("oo1")).toBeUndefined();
  });

  it("14. reject drops the offer row", async () => {
    await boot();
    harness.emit({ offer_received: { id: "o2",
      from: "p",
      name: "f",
      size: 1,
      note: "",
      sender_name: "" } });
    await useStore.getState().reject("o2");
    expect(harness.recorder.reject).toEqual(["o2"]);
    expect(row("oo2")).toBeUndefined();
  });

  it("15. send forwards recipient, paths and note", async () => {
    await boot();
    await useStore.getState().send("proj", ["/a.txt", "/b.txt"], "ciao");
    expect(harness.recorder.sendTo).toEqual([["proj", ["/a.txt", "/b.txt"], "ciao"]]);
  });

  it("16. cancel shows it landed, then the engine's verdict wins", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "d", total_size: 1 } });
    harness.emit({ deposited: { id: 1 } });

    const done = useStore.getState().cancel(1);
    expect(row("t1").status).toBe("cancelling"); // optimistic, immediately
    await done;
    expect(harness.recorder.cancel).toEqual([1]);

    harness.emit({ cancelled: { id: 1 } });
    expect(row("t1").status).toBe("cancelled");
  });

  it("17. a refused cancel puts the row back — it must not pretend", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "d", total_size: 1 } });
    harness.emit({ deposited: { id: 1 } });
    harness.fail = new Set(["cancel"]);

    await expect(useStore.getState().cancel(1)).rejects.toThrow();
    expect(row("t1").status).toBe("deposited");
  });

  it("18. a refused remove keeps the row — the list must match the daemon", async () => {
    // The daemon refuses to drop a transfer that is still in flight. Swallowing that
    // and dropping the row anyway is what made the GUI lie about a cancelled send.
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "d", total_size: 1 } });
    harness.fail = new Set(["remove"]);

    await expect(useStore.getState().removeRow("t1")).rejects.toThrow();
    expect(row("t1")).toBeTruthy();
  });

  it("19. an accepted remove drops the row on both sides", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "d", total_size: 1 } });
    harness.emit({ completed: { id: 1, path: null } });
    await useStore.getState().removeRow("t1");
    expect(harness.recorder.remove).toEqual([1]);
    expect(row("t1")).toBeUndefined();
  });

  it("20. verifying a contact tells the daemon and re-stamps the badge", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "peer1", verified: false })];
    harness.snapshot.transfers = [dto.transfer({ id: 1, peer: "peer1" })];
    await boot();
    expect(row("t1").verified).toBe(false);

    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "peer1", verified: true })];
    await useStore.getState().markVerified("proj");

    expect(harness.recorder.markVerified).toEqual(["proj"]);
    expect(row("t1").verified).toBe(true);
  });

  it("21. pause-all pauses only what is running, and resumes what it paused", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "a", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "send", name: "b", total_size: 1 } });
    harness.emit({ completed: { id: 2, path: null } }); // finished: leave alone

    await useStore.getState().togglePauseAll();
    expect(harness.recorder.pause).toEqual([1]);

    harness.emit({ paused: { id: 1, reason: "x" } });
    await useStore.getState().togglePauseAll();
    expect(harness.recorder.resume).toEqual([1]);
  });
});

describe("list housekeeping", () => {
  it("22. clearFinished keeps everything still in flight", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "live", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "send", name: "done", total_size: 1 } });
    harness.emit({ completed: { id: 2, path: null } });
    harness.emit({ started: { id: 3, direction: "send", name: "held", total_size: 1 } });
    harness.emit({ deposited: { id: 3 } });

    await useStore.getState().clearFinished();
    expect(row("t1")).toBeTruthy();
    expect(row("t3")).toBeTruthy(); // awaiting pickup is NOT finished
    expect(row("t2")).toBeUndefined();
    // Daemon-side too — a local-only sweep would see every row come back with
    // the next snapshot.
    expect(harness.recorder.clearFinished).toBe(1);
  });

  it("23. moveItem swaps a row with its neighbour, and stops at the edges", async () => {
    await boot();
    harness.emit({ started: { id: 1, direction: "send", name: "first", total_size: 1 } });
    harness.emit({ started: { id: 2, direction: "send", name: "second", total_size: 1 } });

    const order = () =>
      Object.values(useStore.getState().transfers)
        .sort((a, b) => b.rank - a.rank)
        .map((t) => t.name);
    const before = order();

    useStore.getState().moveItem("t1", -1); // already at an edge for its direction
    useStore.getState().moveItem(before[0] === "first" ? "t1" : "t2", 1);
    expect(order()).toEqual([...before].reverse());
  });

  it("24. the peer label prefers a saved contact over a raw id", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "peer1" })];
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, peer: "peer1" }),
      dto.transfer({ id: 2, peer: "unknownlongidentifier1234567890" }),
    ];
    await boot();
    expect(row("t1").peer).toBe("proj");
    expect(row("t2").peer).toContain("…"); // shortened id
  });
});

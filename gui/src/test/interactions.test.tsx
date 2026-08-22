// Clicks. Not the store behind them — the actual controls, driven the way a user
// drives them.
//
// This layer exists because of the "Accetta does nothing" bug. The store tests
// called `await store.accept(...)`; the button calls `store.accept(...)` with no
// await and no catch, from an `onClick` that cannot await anything. A refusal
// therefore vanished into the event loop — invisible to a test that awaited the
// promise itself, and to the user. Testing the function is not testing the button.
//
// So: press every control, and assert what the user would see happen.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { dto, harness, makeIpcMock, resetHarness, pick } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
const dialogOpen = vi.fn(() => Promise.resolve(null as unknown));
const dialogSave = vi.fn(() => Promise.resolve(null as unknown));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...a: unknown[]) => dialogOpen(...(a as [])),
  save: (...a: unknown[]) => dialogSave(...(a as [])),
}));
const revealItemInDir = vi.fn(() => Promise.resolve());
const openUrl = vi.fn(() => Promise.resolve());
// No `openPath` here: opening a file goes through the bridge (`api.openPath`),
// not the plugin — see the comment on the command in `bridge.rs`.
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (...a: unknown[]) => revealItemInDir(...(a as [])),
  openUrl: (...a: unknown[]) => openUrl(...(a as [])),
}));

import { useStore } from "../store";
import { App } from "../App";
import { useToasts } from "../ui/Toasts";

function fresh() {
  useStore.setState({
    connected: true,
    status: {
      version: "0.9.2",
      public_id: "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha",
      fingerprint: "able-otter-nine",
      relay: "https://relay.test",
      transfers: 0,
      pending: 0,
      download_dir: "/Users/ls/Arvolo",
      display_name: "",
    },
    guiVersion: "0.9.2",
    loadError: null,
    actionError: null,
    contacts: [],
    contactsById: {},
    transfers: {},
    search: "",
    pauseAll: false,
    sheetPicks: null,
    sheetTo: null,
    incomingOfferId: null,
    personOpen: null,
    paletteOpen: false,
    deposits: [],
    depositsLoading: false,
    depositsError: null,
    revoking: [],
    pairing: null,
    config: null,
    sync: null,
    // A test that navigated away must not leave the next one on the wrong screen.
    route: "transfers",
    // Nor may it leave its answer about who was online: the dots are the one
    // thing here that is polled, so a stale one renders before anything is asked.
    presence: {},
    presenceLoading: false,
    receiveOpen: false,
  });
  useToasts.setState({ items: [] });
}

/** Seed the daemon's snapshot with one pending offer and mount the whole app,
 *  waiting for `init()`'s reload to land. Setting the store directly is not enough
 *  here: mounting `App` refetches, and would wipe a hand-placed row. */
async function renderAppWithOffer() {
  harness.snapshot.pending = [
    dto.offer({ id: "o1", from: "peer1", name: "arrivo.zip" }),
  ];
  render(<App />);
  await screen.findByText("arrivo.zip");
}

/** Same, for a send that is under way. */
async function renderAppWithSend(over: Record<string, unknown> = {}) {
  harness.snapshot.transfers = [
    dto.transfer({ id: 1, name: "invio.txt", status: "active", ...(over as object) }),
  ];
  render(<App />);
  await screen.findByText("invio.txt");
}

/** A download under way from someone the address book has never heard of. */
async function renderAppWithDownload(over: Record<string, unknown> = {}) {
  harness.snapshot.transfers = [
    dto.transfer({
      id: 7,
      direction: "recv",
      name: "arrivo.zip",
      peer: "peer9",
      status: "active",
      ...(over as object),
    }),
  ];
  render(<App />);
  await screen.findByText("arrivo.zip");
}

/** The row's overflow menu, opened. Every row action lives behind it. */
async function openRowMenu(name: string) {
  fireEvent.click(await screen.findByLabelText(`Actions for ${name}`));
  return screen.findByRole("menu");
}

/** The text of every toast currently on screen. Refusals land here now, rather
 *  than in a banner: the store records them and `App` re-raises them. */
function toastText(): string {
  return document.querySelector(".toasts")?.textContent ?? "";
}

/** Let a fake-timer test come to rest. Two things make this necessary and
 *  neither is optional: RTL's `findBy*` never settles once the clock is faked
 *  (it waits on the clock it just stopped), and advancing timers synchronously
 *  leaves React's scheduled render unflushed — the state is updated but the DOM
 *  is not. `advanceTimersByTimeAsync` yields to the real event loop between
 *  callbacks, and `act` flushes what that scheduled. */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  resetHarness();
  fresh();
  dialogOpen.mockClear();
  dialogSave.mockClear();
  revealItemInDir.mockClear();
  openUrl.mockClear();
});
afterEach(cleanup);

// ---------------------------------------------------------------------------

describe("an incoming offer", () => {
  it("102. opening the row and pressing Accetta tells the daemon", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accept and download"));
    await waitFor(() =>
      expect(harness.recorder.accept).toEqual([["o1", null, null]])
    );
  });

  it("103. when the daemon refuses, the user is told — not left guessing", async () => {
    await renderAppWithOffer();
    harness.fail = new Set(["acceptOffer"]);
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accept and download"));
    await waitFor(() => expect(toastText()).toMatch(/reject|didn't work/i));
  });

  it("104. a refused accept keeps the row, so it can be retried", async () => {
    await renderAppWithOffer();
    harness.fail = new Set(["acceptOffer"]);
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accept and download"));
    await waitFor(() => expect(toastText()).toBeTruthy());
    expect(useStore.getState().transfers["oo1"]).toBeTruthy();
  });

  it("106. Reject tells the daemon to reject", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Reject"));
    await waitFor(() => expect(harness.recorder.reject).toEqual(["o1"]));
  });

  it("107. a refused reject reports itself and keeps the row", async () => {
    await renderAppWithOffer();
    harness.fail = new Set(["rejectOffer"]);
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Reject"));
    await waitFor(() => expect(toastText()).toBeTruthy());
    expect(useStore.getState().transfers["oo1"]).toBeTruthy();
  });

  it("108. clicking an arrival opens the decision dialog", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    expect(await screen.findByText("Somebody is sending you a file")).toBeTruthy();
  });

  it("109. an ordinary transfer does not — there is nothing to confirm", async () => {
    await renderAppWithSend();
    fireEvent.click(screen.getByText("invio.txt"));
    expect(screen.queryByText("Somebody is sending you a file")).toBeNull();
  });

  it("a password-protected deposit asks, keeps the offer, and retries", async () => {
    // Nothing in an offer says it is protected — the fetch refusing is the only
    // signal — so the field appears after the first attempt, not before it.
    harness.fail = new Set(["acceptOffer:password"]);
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    expect(screen.queryByLabelText("Password")).toBeNull();

    fireEvent.click(await screen.findByText("Accept and download"));
    const field = await screen.findByLabelText("Password");
    // The offer must survive the refusal, or there is nothing left to retry.
    expect(useStore.getState().transfers["oo1"]).toBeTruthy();

    fireEvent.change(field, { target: { value: "segreto" } });
    fireEvent.click(screen.getByText("Accept and download"));
    await waitFor(() =>
      expect(
        harness.recorder.accept[harness.recorder.accept.length - 1]
      ).toEqual(["o1", null, "segreto"])
    );
  });

  it("135. it shows the sender's note", async () => {
    harness.snapshot.pending = [
      dto.offer({ id: "o1", from: "peer1", name: "arrivo.zip", note: "guarda qui" }),
    ];
    render(<App />);
    fireEvent.click(await screen.findByText("arrivo.zip"));
    expect(await screen.findByText("guarda qui")).toBeTruthy();
  });

  it("137. it names the real download folder, not a guess", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    await screen.findByText("Somebody is sending you a file");
    expect(document.body.textContent).toContain("/Users/ls/Arvolo");
  });

  it("134. Accetta without picking a folder uses the daemon's default", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accept and download"));
    await waitFor(() =>
      expect(harness.recorder.accept[0][1]).toBeNull()
    );
  });

  it("133. Accetta uses the folder the user picked", async () => {
    // The folder arrives from the native Rust-side picker as an id plus a display
    // name; what the accept command receives is the id, and resolving it back to
    // a path happens where the webview cannot reach.
    const { api } = await import("../ipc");
    const dir = pick("qui", { isDir: true });
    const spy = vi.spyOn(api, "pickFiles").mockResolvedValue([dir]);
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Choose…"));
    await waitFor(() => expect(spy).toHaveBeenCalledWith(true));
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Destination folder") as HTMLInputElement)
          .value
      ).toBe("qui")
    );
    fireEvent.click(screen.getByText("Accept and download"));
    await waitFor(() =>
      expect(harness.recorder.accept).toEqual([["o1", dir.id, null]])
    );
  });
});

// ---------------------------------------------------------------------------

describe("the row menu", () => {
  it("110. it opens, and its actions reach the daemon", async () => {
    await renderAppWithSend();
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Pause"));
    await waitFor(() => expect(harness.recorder.pause).toEqual([1]));
  });

  it("111. a refused menu action reports itself", async () => {
    await renderAppWithSend();
    harness.fail = new Set(["pause"]);
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Pause"));
    await waitFor(() => expect(toastText()).toBeTruthy());
  });

  it("112. Resume resumes a paused transfer", async () => {
    await renderAppWithSend({ status: "paused: dall'utente" });
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Resume"));
    await waitFor(() => expect(harness.recorder.resume).toEqual([1]));
  });

  it("113. Cancel asks first, then cancels — it is not undoable", async () => {
    await renderAppWithSend();
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Cancel"));
    // The confirm stands between the click and the daemon.
    expect(harness.recorder.cancel).toEqual([]);
    fireEvent.click(await screen.findByText("Cancel the transfer"));
    await waitFor(() => expect(harness.recorder.cancel).toEqual([1]));
  });

  it("114. Take off the list removes a concluded one", async () => {
    await renderAppWithSend({ status: "completed" });
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Take off the list"));
    await waitFor(() => expect(harness.recorder.remove).toEqual([1]));
  });

  it("115. a refused removal keeps the row on screen", async () => {
    await renderAppWithSend({ status: "completed" });
    harness.fail = new Set(["remove"]);
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Take off the list"));
    await waitFor(() => expect(toastText()).toBeTruthy());
    expect(screen.getByText("invio.txt")).toBeTruthy();
  });

  it("116. Open the folder reveals a completed download in the file manager", async () => {
    await renderAppWithSend({ status: "completed" });
    // `path` only ever arrives on the `completed` event, never in a snapshot.
    useStore.setState((s) => ({
      transfers: {
        ...s.transfers,
        t1: { ...s.transfers.t1, path: "/Users/ls/Arvolo/invio.txt" },
      },
    }));
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Open the folder"));
    await waitFor(() =>
      expect(revealItemInDir).toHaveBeenCalledWith("/Users/ls/Arvolo/invio.txt")
    );
  });

  // The offer dialog asks the same question, but only if you go through it: an
  // auto-accepted arrival, or one waved past, otherwise leaves the sender
  // nameless — with their id nowhere but a tooltip.
  it("117a. a download from a stranger offers to save them, and saves what you type", async () => {
    await renderAppWithDownload();
    const menu = await openRowMenu("arrivo.zip");
    fireEvent.click(within(menu).getByText("Save the sender…"));
    const field = await screen.findByLabelText("Name to give the contact");
    fireEvent.change(field, { target: { value: "lorenzo" } });
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() =>
      expect(harness.recorder.addContact).toEqual([["lorenzo", "peer9"]])
    );
  });

  it("117b. someone already in the address book is not offered again", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "peer9" })];
    await renderAppWithDownload();
    const menu = await openRowMenu("arrivo.zip");
    expect(within(menu).queryByText("Save the sender…")).toBeNull();
  });

  // Their own name for themselves is a claim, not evidence — but it is the best
  // first guess at what to call them, and typing it out again is the friction
  // that stops people saving anyone at all.
  it("117c. the name the sender claims is offered as the default", async () => {
    await renderAppWithDownload({ sender_name: "Anna Maria" });
    const menu = await openRowMenu("arrivo.zip");
    fireEvent.click(within(menu).getByText("Save the sender…"));
    const field = (await screen.findByLabelText(
      "Name to give the contact"
    )) as HTMLInputElement;
    expect(field.value).toBe("anna-maria");
  });

  it("118. the arrow keys on a row's grip reorder without touching the daemon", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "primo.txt", status: "active" }),
      dto.transfer({ id: 2, name: "secondo.txt", status: "active" }),
    ];
    render(<App />);
    await screen.findByText("primo.txt");
    const order = () =>
      Object.values(useStore.getState().transfers)
        .sort((a, b) => b.rank - a.rank)
        .map((t) => t.name);
    const [top, below] = order();
    // The drag gesture has no keyboard, so the grip carries one: same move,
    // one step at a time. Without it the board would be unorderable without a
    // pointer, which is what the menu items used to cover.
    const grip = screen.getByLabelText(
      `Move ${below}: drag, or use the up and down arrows`
    );
    fireEvent.keyDown(grip, { key: "ArrowUp" });
    await waitFor(() => expect(order()[0]).toBe(below));
    expect(order()[1]).toBe(top);
    expect(harness.recorder.cancel).toEqual([]);
  });

  it("119. dragging a grip drops the row where it was let go", async () => {
    harness.snapshot.transfers = [1, 2, 3].map((id) =>
      dto.transfer({ id, name: `f${id}.txt`, status: "active" })
    );
    render(<App />);
    await screen.findByText("f1.txt");
    const order = () =>
      Object.values(useStore.getState().transfers)
        .sort((a, b) => b.rank - a.rank)
        .map((t) => t.name);
    const [top, mid, bottom] = order();

    const grip = screen.getByLabelText(
      `Move ${top}: drag, or use the up and down arrows`
    );
    // jsdom lays nothing out — every rect is 0×0 — so the drag has to be given
    // the geometry it would read from a real window: three rows, 60px each.
    const rows = Array.from(
      grip.closest(".rows")!.querySelectorAll<HTMLElement>(":scope > .row")
    );
    expect(rows).toHaveLength(3);
    rows.forEach((el, i) => {
      el.getBoundingClientRect = () =>
        ({ top: i * 60, height: 60, bottom: i * 60 + 60 }) as DOMRect;
    });

    fireEvent.pointerDown(grip, { button: 0, clientY: 30 });
    // Past the centre of the third row (150), so it lands last.
    fireEvent.pointerMove(window, { clientY: 195 });
    fireEvent.pointerUp(window, { clientY: 195 });
    await waitFor(() => expect(order()).toEqual([mid, bottom, top]));
  });

  it("120. a drag let go on Escape leaves the order alone", async () => {
    harness.snapshot.transfers = [1, 2].map((id) =>
      dto.transfer({ id, name: `g${id}.txt`, status: "active" })
    );
    render(<App />);
    await screen.findByText("g1.txt");
    const order = () =>
      Object.values(useStore.getState().transfers)
        .sort((a, b) => b.rank - a.rank)
        .map((t) => t.name);
    const before = order();

    const grip = screen.getByLabelText(
      `Move ${before[0]}: drag, or use the up and down arrows`
    );
    const rows = Array.from(
      grip.closest(".rows")!.querySelectorAll<HTMLElement>(":scope > .row")
    );
    rows.forEach((el, i) => {
      el.getBoundingClientRect = () =>
        ({ top: i * 60, height: 60, bottom: i * 60 + 60 }) as DOMRect;
    });

    fireEvent.pointerDown(grip, { button: 0, clientY: 30 });
    fireEvent.pointerMove(window, { clientY: 120 });
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.pointerUp(window, { clientY: 120 });
    expect(order()).toEqual(before);
  });
});

// ---------------------------------------------------------------------------

describe("the board's own controls", () => {
  it("122. typing in the search filters the board", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "trovami.txt", status: "active" }),
      dto.transfer({ id: 2, name: "nascondimi.txt", status: "active" }),
    ];
    render(<App />);
    await screen.findByText("trovami.txt");
    fireEvent.change(screen.getByLabelText("Filter the transfers"), {
      target: { value: "trovami" },
    });
    await waitFor(() => expect(screen.queryByText("nascondimi.txt")).toBeNull());
    expect(screen.getByText("trovami.txt")).toBeTruthy();
  });

  it("123. Pulisci drops the finished rows, daemon-side first", async () => {
    await renderAppWithSend({ status: "completed" });
    fireEvent.click(await screen.findByText(/^Clear/));
    await waitFor(() => expect(harness.recorder.clearFinished).toBe(1));
  });

  it("a pending arrival is announced from any other screen", async () => {
    await renderAppWithOffer();
    useStore.getState().go("history");
    expect(
      await screen.findByText(/wants to send you a file/i)
    ).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------

describe("the send panel", () => {
  const openSend = async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("Send…"));
    return screen.findByText("What you are sending");
  };

  it("124. Files… asks the backend to open the native picker", async () => {
    // The picker moved to the Rust side on purpose: the webview asks for "a
    // pick" and gets back opaque ids, never paths. What this pins is that the
    // button reaches that command — and that what it returns lands in the list
    // under the display name the backend chose.
    const { api } = await import("../ipc"); // the mocked module
    const spy = vi
      .spyOn(api, "pickFiles")
      .mockResolvedValue([pick("scelto.txt")]);
    await openSend();
    fireEvent.click(screen.getByText("Files…"));
    await waitFor(() => expect(spy).toHaveBeenCalledWith(false));
    expect(await screen.findByText("scelto.txt")).toBeDefined();
  });

  it("a drop arrives as registered items from the window, and opens the sheet", async () => {
    // The webview's own drag-drop payload is ignored by design: what opens the
    // sheet is the `files://picked` event the Rust window handler emits after
    // registering the paths. This drives that exact channel.
    render(<App />);
    await act(async () => {
      harness.dropFiles([pick("lanciato.pdf")]);
    });
    expect(await screen.findByText("What you are sending")).toBeDefined();
    expect(screen.getByText("lanciato.pdf")).toBeDefined();
  });

  it("a send hands the daemon registry ids, never paths", async () => {
    // The whole point of the picked-file registry: the only thing the webview can
    // name is an id the backend minted. If a path ever shows up here again, the
    // boundary has moved back to the wrong side.
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt"), pick("b.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Send"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.sendTo).toEqual([
        ["proj", [pick("a.txt").id, pick("b.txt").id], ""],
      ])
    );
    for (const sent of harness.recorder.sendTo[0][1]) {
      expect(sent).not.toContain("/");
    }
  });

  it("125. picking a contact and sending reaches the daemon", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    // "Send" is also the header's own send button; the one under test is the
    // sheet's submit.
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Send"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.sendTo[0]?.[0]).toBe("proj")
    );
  });

  it("126. the note typed in rides along with the send", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.change(
      screen.getByPlaceholderText("Here are the files we talked about."),
      { target: { value: "eccolo" } }
    );
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Send"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.sendTo[0]?.[2]).toBe("eccolo")
    );
  });

  it("127. a refused send says why instead of closing silently", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    harness.fail = new Set(["sendTo"]);
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Send"
      )
    );
    await waitFor(() => expect(toastText()).toBeTruthy());
    // Still open, so the user can change something and retry.
    expect(useStore.getState().sheetPicks).not.toBeNull();
  });

  it("129. the Ticket mode mints a ticket and shows it", async () => {
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("Ticket"));
    fireEvent.click(screen.getByText("Create the ticket"));
    expect(await screen.findByText("arvc-test")).toBeTruthy();
  });

  it("130. the Link mode creates a link and shows it", async () => {
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("Link"));
    fireEvent.click(screen.getByText("Create the link"));
    expect(
      await screen.findByText("https://relay.test/dl/abc#key")
    ).toBeTruthy();
  });

  it("131. a relay that refuses links says so instead of showing nothing", async () => {
    harness.fail = new Set(["createLink"]);
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("Link"));
    fireEvent.click(screen.getByText("Create the link"));
    await waitFor(() => expect(toastText()).toBeTruthy());
  });

  it("the Code mode shows a code to read out", async () => {
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("Code"));
    fireEvent.click(screen.getByText("Generate the code"));
    expect(await screen.findByText("4821-crater-mango")).toBeTruthy();
  });

  it("mailbox options only appear once the send is a deposit", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    await screen.findByText("proj");
    // TTL and password protect a *deposit*; offering them on a live send would
    // promise something that is not being applied.
    expect(screen.queryByText("Expires after")).toBeNull();
    fireEvent.click(screen.getByLabelText("Leave it in the mailbox"));
    expect(await screen.findByText("Expires after")).toBeTruthy();
  });

  it("a deposit send carries its ttl, cap and password to the daemon", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Leave it in the mailbox"));
    fireEvent.change(await screen.findByPlaceholderText("none"), {
      target: { value: "segreto" },
    });
    fireEvent.click(screen.getByText("Leave it in their mailbox"));
    await waitFor(() => expect(harness.recorder.depositTo).toHaveLength(1));
    const [to, , ttl, max, password] = harness.recorder.depositTo[0];
    expect(to).toBe("proj");
    expect(ttl).toBe(7 * 24 * 3600);
    expect(max).toBe(1);
    expect(password).toBe("segreto");
  });

  it("a deposit hands back the arvm… ticket for hand-delivery", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Leave it in the mailbox"));
    fireEvent.click(await screen.findByText("Leave it in their mailbox"));
    expect(await screen.findByText("arvm-test")).toBeTruthy();
  });

  it("reopening the panel never inherits the last send's password", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Leave it in the mailbox"));
    fireEvent.change(await screen.findByPlaceholderText("none"), {
      target: { value: "segreto" },
    });
    useStore.getState().closeSheet();
    useStore.getState().openSheet([pick("b.txt")]);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Leave it in the mailbox"));
    // A password carried over would silently protect a different file for a
    // different person.
    expect(
      (
        (await screen.findByPlaceholderText("none")) as HTMLInputElement
      ).value
    ).toBe("");
  });

  it("132. the ✕ closes the panel without sending", async () => {
    useStore.setState({ sheetPicks: [pick("a.txt")] });
    render(<App />);
    fireEvent.click(await screen.findByLabelText("Close"));
    expect(useStore.getState().sheetPicks).toBeNull();
    expect(harness.recorder.sendTo).toEqual([]);
  });
});

// ---------------------------------------------------------------------------

describe("receiving from a pasted artefact", () => {
  it("recognises a pairing code and sends it to the daemon", async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("Receive…"));
    fireEvent.change(await screen.findByPlaceholderText("4821-crater-mango"), {
      target: { value: "4821-crater-mango" },
    });
    expect(
      screen.getByText(/Send code/)
    ).toBeTruthy();
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Receive"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.recv[0]?.[0]).toBe("4821-crater-mango")
    );
  });

  it("offers a password field only where a password can exist", async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("Receive…"));
    const field = await screen.findByPlaceholderText("4821-crater-mango");
    fireEvent.change(field, { target: { value: "4821-crater-mango" } });
    // A pairing code has no password; suggesting otherwise teaches a falsehood.
    expect(screen.queryByLabelText(/Password/)).toBeNull();
    fireEvent.change(field, { target: { value: "arvm1234" } });
    await waitFor(() =>
      expect(screen.getByText(/Mailbox ticket/)).toBeTruthy()
    );
  });
});

// ---------------------------------------------------------------------------

describe("the address book", () => {
  it("an unreachable relay reads as unknown, never as offline", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "p1" })];
    harness.fail = new Set(["presence"]);
    render(<App />);
    useStore.getState().go("people");
    await screen.findByText("proj");
    // Collapsing "could not ask" into "offline" is what makes a dead relay look
    // exactly like everyone having gone home.
    expect(await screen.findByLabelText("Presence unknown")).toBeTruthy();
    expect(screen.queryByLabelText("Not connected")).toBeNull();
  });

  it("a contact the relay reports as present is marked online", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "p1" })];
    harness.snapshot.presence = { p1: true };
    render(<App />);
    useStore.getState().go("people");
    expect(await screen.findByLabelText("Connected")).toBeTruthy();
  });

  it("the dots keep up on their own while the screen stays open", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "p1" })];
    harness.snapshot.presence = { p1: true };
    vi.useFakeTimers();
    try {
      render(<App />);
      useStore.getState().go("people");
      await settle();
      expect(screen.getByLabelText("Connected")).toBeTruthy();
      // They hang up. Nobody touches the window: a dot that only refreshed on a
      // click would still be claiming they are there.
      harness.snapshot.presence = { p1: false };
      await settle(60_000);
      expect(screen.getByLabelText("Not connected")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a window nobody is looking at stops asking", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "p1" })];
    harness.snapshot.presence = { p1: true };
    // jsdom keeps `hidden` on Document.prototype, where a spy does not reliably
    // land; an own property does.
    let hidden = true;
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => hidden,
    });
    vi.useFakeTimers();
    try {
      render(<App />);
      useStore.getState().go("people");
      await settle();
      harness.snapshot.presence = { p1: false };
      await settle(60_000);
      // A tick passes and nothing is asked: polling a relay for dots nobody can
      // see is just noise on someone's uplink.
      expect(screen.queryByLabelText("Not connected")).toBeNull();
      // Coming back is the moment the answer matters most, so it is asked for
      // then rather than at the next tick.
      hidden = false;
      window.dispatchEvent(new Event("focus"));
      await settle();
      expect(screen.getByLabelText("Not connected")).toBeTruthy();
    } finally {
      vi.useRealTimers();
      delete (document as unknown as { hidden?: boolean }).hidden;
    }
  });

  it("Send from a person row opens the sheet already addressed to them", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(
      within((await screen.findByText("proj")).closest(".row") as HTMLElement)
        .getByText("Send")
    );
    // Choosing a person and then being asked to choose them again is the app
    // forgetting what it was just told.
    expect(useStore.getState().sheetTo).toBe("proj");
    // The recipient is already chosen, so the only thing still missing is the
    // payload — and the submit stays disabled until there is one.
    const submit = within(
      document.querySelector(".sheet-foot") as HTMLElement
    ).getByText("Send").closest("button") as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    useStore.getState().openSheet([pick("a.txt")], "proj");
    await waitFor(() => expect(submit.disabled).toBe(false));
    fireEvent.click(submit);
    await waitFor(() => expect(harness.recorder.sendTo[0]?.[0]).toBe("proj"));
  });

  it("Create a link opens the sheet already set to Link, not to a contact", async () => {
    render(<App />);
    useStore.getState().go("deposits");
    fireEvent.click((await screen.findAllByText("Create a link"))[0]);
    // The section only makes links, so asking which way to send would be asking
    // a question the click already answered.
    await waitFor(() => expect(useStore.getState().sheetMode).toBe("link"));
    const sheet = document.querySelector(".sheet") as HTMLElement;
    const chosen = within(sheet)
      .getByText("Link")
      .closest("button") as HTMLButtonElement;
    expect(chosen.getAttribute("aria-checked") ?? chosen.getAttribute("aria-selected"))
      .toBe("true");
  });

  it("a mode preselection does not survive into the next send", async () => {
    render(<App />);
    useStore.getState().openSheet([], undefined, "link");
    await waitFor(() => expect(useStore.getState().sheetMode).toBe("link"));
    useStore.getState().closeSheet();
    // Reopening from somewhere that implies nothing must start on a contact,
    // or the previous errand quietly decides this one.
    useStore.getState().openSheet([]);
    await waitFor(() => expect(useStore.getState().sheetMode).toBe(null));
  });

  it("117. a contact's fingerprint is reachable, and verifying is deliberate", async () => {
    harness.snapshot.contacts = [
      dto.contact({ name: "proj", verified: false }),
    ];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByText("Details"));
    await screen.findByText("Fingerprint");
    const button = screen.getByText("Mark as verified");
    // Reading the fingerprint and asserting you checked it are two acts.
    expect((button.closest("button") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(
      screen.getByLabelText(/I compared the fingerprint/, { selector: "input" })
    );
    fireEvent.click(button);
    await waitFor(() =>
      expect(harness.recorder.markVerified).toEqual(["proj"])
    );
  });

  it("trusting an unverified contact demands an explicit override", async () => {
    harness.snapshot.contacts = [
      dto.contact({ name: "proj", verified: false }),
    ];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByLabelText("Actions for proj"));
    fireEvent.click(
      within(await screen.findByRole("menu")).getByText(/Mark as trusted/)
    );
    // Not sent yet: auto-downloading from an unconfirmed key is a MITM risk.
    expect(harness.recorder.markTrusted).toEqual([]);
    fireEvent.click(await screen.findByText("Force it anyway"));
    await waitFor(() =>
      expect(harness.recorder.markTrusted).toEqual([["proj", true]])
    );
  });

  it("a verified contact is trusted without a warning", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", verified: true })];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByLabelText("Actions for proj"));
    fireEvent.click(
      within(await screen.findByRole("menu")).getByText(/Mark as trusted/)
    );
    await waitFor(() =>
      expect(harness.recorder.markTrusted).toEqual([["proj", false]])
    );
  });
});

// ---------------------------------------------------------------------------

describe("pairing", () => {
  it("hosting a contact exchange starts a session and shows its code", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click((await screen.findAllByText("Swap contacts"))[0]);
    await waitFor(() =>
      expect(harness.recorder.startPairing[0]?.[0]).toBe("contact_host")
    );
    // The code arrives as an event, not as the call's return value.
    useStore.getState().applyEvent({
      type: "pairing_code",
      session: "pair-1",
      code: "4821-crater-mango",
    });
    expect(await screen.findByText("4821-crater-mango")).toBeTruthy();
  });

  it("joining opens the code input without contacting the daemon first", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByText("I have a pairing code"));
    // Starting a session now would spawn one that fails instantly ("a pairing
    // code is required") and replace the very input the user is meant to type
    // into with a spinner.
    expect(harness.recorder.startPairing).toEqual([]);
    const field = await screen.findByPlaceholderText("4821-crater-mango");
    fireEvent.change(field, { target: { value: "4821-crater-mango" } });
    fireEvent.click(screen.getByText("Link"));
    await waitFor(() =>
      expect(harness.recorder.startPairing[0]).toEqual([
        "contact_join",
        null,
        "4821-crater-mango",
      ])
    );
  });

  it("starting a second exchange retires the first", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click((await screen.findAllByText("Swap contacts"))[0]);
    await waitFor(() => expect(useStore.getState().pairing?.session).toBe("pair-1"));
    // The hosting side waits until cancelled, so an orphaned session would keep
    // its rendezvous slot with no handle left anywhere to stop it.
    await useStore.getState().startPairing("device_host");
    expect(harness.recorder.cancelPairing).toContain("pair-1");
  });

  it("an outcome that beats its own session handle is not lost", async () => {
    // The daemon spawns the session and *then* writes the reply naming it, so a
    // fast failure can genuinely arrive first.
    render(<App />);
    useStore.setState({
      pairing: {
        session: null,
        kind: "device_host",
        code: "",
        phase: "starting",
        message: "",
        needsRestart: false,
      },
    });
    useStore.getState().applyEvent({
      type: "pairing_failed",
      session: "pair-1",
      kind: "device_host",
      error: "nessun relay configurato",
      cancelled: false,
    });
    // Held, not dropped: the sheet would otherwise spin for ever.
    expect(useStore.getState().pairing?.phase).toBe("starting");
    await useStore.getState().startPairing("device_host");
    await waitFor(() =>
      expect(useStore.getState().pairing?.phase).toBe("failed")
    );
    expect(useStore.getState().pairing?.message).toBe("nessun relay configurato");
  });

  it("closing the sheet cancels the session on the daemon", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click((await screen.findAllByText("Swap contacts"))[0]);
    await waitFor(() => expect(useStore.getState().pairing?.session).toBe("pair-1"));
    fireEvent.click(screen.getByLabelText("Close"));
    await waitFor(() =>
      expect(harness.recorder.cancelPairing).toEqual(["pair-1"])
    );
  });

  it("a cancelled exchange is not reported back as an error", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByText("I have a pairing code"));
    await screen.findByText("Code");
    useStore.setState({
      pairing: {
        session: "pair-1",
        kind: "contact_join",
        code: "x",
        phase: "waiting",
        message: "",
        needsRestart: false,
      },
    });
    useStore.getState().applyEvent({
      type: "pairing_failed",
      session: "pair-1",
      kind: "contact_join",
      error: "pairing cancelled",
      cancelled: true,
    });
    expect(await screen.findByText("Cancelled.")).toBeTruthy();
  });

  it("a device join that replaced the identity demands a daemon restart", async () => {
    render(<App />);
    useStore.setState({
      pairing: {
        session: "pair-9",
        kind: "device_join",
        code: "x",
        phase: "waiting",
        message: "",
        needsRestart: false,
      },
    });
    useStore.getState().applyEvent({
      type: "pairing_done",
      session: "pair-9",
      kind: "device_join",
      summary: "Connected.",
      needs_restart: true,
    });
    expect(await screen.findByText("Restart and close")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------

describe("settings", () => {
  it("a relay forced by the environment is shown but not editable", async () => {
    harness.snapshot.config = {
      ...harness.snapshot.config,
      relay_source: "env",
      relay: "https://forced.test",
    };
    render(<App />);
    useStore.getState().go("settings");
    const field = (await screen.findByLabelText("Relay")) as HTMLInputElement;
    // Editing it could not win, so it must not look like it could.
    expect(field.disabled).toBe(true);
  });

  it("clearing the display name sends a clear, not an empty string", async () => {
    harness.snapshot.config = {
      ...harness.snapshot.config,
      display_name: "Luca",
    };
    render(<App />);
    useStore.getState().go("settings");
    const field = (await screen.findByLabelText(
      "The name you show"
    )) as HTMLInputElement;
    fireEvent.change(field, { target: { value: "" } });
    fireEvent.click(
      within(field.closest(".hstack-sm") as HTMLElement).getByText("Save")
    );
    await waitFor(() =>
      expect(harness.recorder.setConfig[0]).toEqual({ display_name: "clear" })
    );
  });
});

// ---------------------------------------------------------------------------

describe("the failure notice", () => {
  it("138. it can be dismissed", async () => {
    render(<App />);
    useStore.setState({ actionError: "something went wrong" });
    fireEvent.click(await screen.findByLabelText("Close"));
    await waitFor(() => expect(toastText()).toBe(""));
  });

  it("an error never disappears on its own — it has to be read", async () => {
    vi.useFakeTimers();
    try {
      render(<App />);
      useStore.setState({ actionError: "boom" });
      await vi.waitFor(() => expect(toastText()).toContain("boom"));
      vi.advanceTimersByTime(30_000);
      expect(toastText()).toContain("boom");
    } finally {
      vi.useRealTimers();
    }
  });
});

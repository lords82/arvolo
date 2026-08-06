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
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

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
const openPath = vi.fn(() => Promise.resolve());
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (...a: unknown[]) => revealItemInDir(...(a as [])),
  openUrl: (...a: unknown[]) => openUrl(...(a as [])),
  openPath: (...a: unknown[]) => openPath(...(a as [])),
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
    sheetPaths: null,
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

/** The row's overflow menu, opened. Every row action lives behind it. */
async function openRowMenu(name: string) {
  fireEvent.click(await screen.findByLabelText(`Azioni per ${name}`));
  return screen.findByRole("menu");
}

/** The text of every toast currently on screen. Refusals land here now, rather
 *  than in a banner: the store records them and `App` re-raises them. */
function toastText(): string {
  return document.querySelector(".toasts")?.textContent ?? "";
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
    fireEvent.click(await screen.findByText("Accetta e scarica"));
    await waitFor(() =>
      expect(harness.recorder.accept).toEqual([["o1", null]])
    );
  });

  it("103. when the daemon refuses, the user is told — not left guessing", async () => {
    await renderAppWithOffer();
    harness.fail = new Set(["acceptOffer"]);
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accetta e scarica"));
    await waitFor(() => expect(toastText()).toMatch(/rifiut|Non è andata/i));
  });

  it("104. a refused accept keeps the row, so it can be retried", async () => {
    await renderAppWithOffer();
    harness.fail = new Set(["acceptOffer"]);
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accetta e scarica"));
    await waitFor(() => expect(toastText()).toBeTruthy());
    expect(useStore.getState().transfers["oo1"]).toBeTruthy();
  });

  it("106. Rifiuta tells the daemon to reject", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Rifiuta"));
    await waitFor(() => expect(harness.recorder.reject).toEqual(["o1"]));
  });

  it("107. a refused reject reports itself and keeps the row", async () => {
    await renderAppWithOffer();
    harness.fail = new Set(["rejectOffer"]);
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Rifiuta"));
    await waitFor(() => expect(toastText()).toBeTruthy());
    expect(useStore.getState().transfers["oo1"]).toBeTruthy();
  });

  it("108. clicking an arrival opens the decision dialog", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    expect(await screen.findByText("Ti stanno mandando un file")).toBeTruthy();
  });

  it("109. an ordinary transfer does not — there is nothing to confirm", async () => {
    await renderAppWithSend();
    fireEvent.click(screen.getByText("invio.txt"));
    expect(screen.queryByText("Ti stanno mandando un file")).toBeNull();
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
    await screen.findByText("Ti stanno mandando un file");
    expect(document.body.textContent).toContain("/Users/ls/Arvolo");
  });

  it("134. Accetta without picking a folder uses the daemon's default", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Accetta e scarica"));
    await waitFor(() =>
      expect(harness.recorder.accept[0][1]).toBeNull()
    );
  });

  it("133. Accetta uses the folder the user picked", async () => {
    dialogOpen.mockImplementation(() => Promise.resolve("/tmp/qui" as unknown));
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("arrivo.zip"));
    fireEvent.click(await screen.findByText("Scegli…"));
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Cartella di destinazione") as HTMLInputElement)
          .value
      ).toBe("/tmp/qui")
    );
    fireEvent.click(screen.getByText("Accetta e scarica"));
    await waitFor(() =>
      expect(harness.recorder.accept).toEqual([["o1", "/tmp/qui"]])
    );
  });
});

// ---------------------------------------------------------------------------

describe("the row menu", () => {
  it("110. it opens, and its actions reach the daemon", async () => {
    await renderAppWithSend();
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Metti in pausa"));
    await waitFor(() => expect(harness.recorder.pause).toEqual([1]));
  });

  it("111. a refused menu action reports itself", async () => {
    await renderAppWithSend();
    harness.fail = new Set(["pause"]);
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Metti in pausa"));
    await waitFor(() => expect(toastText()).toBeTruthy());
  });

  it("112. Riprendi resumes a paused transfer", async () => {
    await renderAppWithSend({ status: "paused: dall'utente" });
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Riprendi"));
    await waitFor(() => expect(harness.recorder.resume).toEqual([1]));
  });

  it("113. Annulla asks first, then cancels — it is not undoable", async () => {
    await renderAppWithSend();
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Annulla"));
    // The confirm stands between the click and the daemon.
    expect(harness.recorder.cancel).toEqual([]);
    fireEvent.click(await screen.findByText("Annulla il trasferimento"));
    await waitFor(() => expect(harness.recorder.cancel).toEqual([1]));
  });

  it("114. Togli dalla lista removes a concluded one", async () => {
    await renderAppWithSend({ status: "completed" });
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Togli dalla lista"));
    await waitFor(() => expect(harness.recorder.remove).toEqual([1]));
  });

  it("115. a refused removal keeps the row on screen", async () => {
    await renderAppWithSend({ status: "completed" });
    harness.fail = new Set(["remove"]);
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Togli dalla lista"));
    await waitFor(() => expect(toastText()).toBeTruthy());
    expect(screen.getByText("invio.txt")).toBeTruthy();
  });

  it("116. Apri la cartella reveals a completed download in the file manager", async () => {
    await renderAppWithSend({ status: "completed" });
    // `path` only ever arrives on the `completed` event, never in a snapshot.
    useStore.setState((s) => ({
      transfers: {
        ...s.transfers,
        t1: { ...s.transfers.t1, path: "/Users/ls/Arvolo/invio.txt" },
      },
    }));
    const menu = await openRowMenu("invio.txt");
    fireEvent.click(within(menu).getByText("Apri la cartella"));
    await waitFor(() =>
      expect(revealItemInDir).toHaveBeenCalledWith("/Users/ls/Arvolo/invio.txt")
    );
  });

  it("118. Sposta giù reorders without touching the daemon", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "primo.txt", status: "active" }),
      dto.transfer({ id: 2, name: "secondo.txt", status: "active" }),
    ];
    render(<App />);
    await screen.findByText("primo.txt");
    const before = useStore.getState().transfers.t2.rank;
    const menu = await openRowMenu("secondo.txt");
    fireEvent.click(within(menu).getByText("Sposta giù"));
    await waitFor(() =>
      expect(useStore.getState().transfers.t2.rank).not.toBe(before)
    );
    expect(harness.recorder.cancel).toEqual([]);
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
    fireEvent.change(screen.getByLabelText("Filtra i trasferimenti"), {
      target: { value: "trovami" },
    });
    await waitFor(() => expect(screen.queryByText("nascondimi.txt")).toBeNull());
    expect(screen.getByText("trovami.txt")).toBeTruthy();
  });

  it("123. Pulisci drops the finished rows, daemon-side first", async () => {
    await renderAppWithSend({ status: "completed" });
    fireEvent.click(await screen.findByText(/^Pulisci/));
    await waitFor(() => expect(harness.recorder.clearFinished).toBe(1));
  });

  it("a pending arrival is announced from any other screen", async () => {
    await renderAppWithOffer();
    useStore.getState().go("history");
    expect(
      await screen.findByText(/vuole mandarti un file/i)
    ).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------

describe("the send panel", () => {
  const openSend = async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("Invia…"));
    return screen.findByText("Cosa mandi");
  };

  it("124. File… opens the OS picker", async () => {
    await openSend();
    fireEvent.click(screen.getByText("File…"));
    await waitFor(() => expect(dialogOpen).toHaveBeenCalled());
  });

  it("125. picking a contact and sending reaches the daemon", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    // "Invia" is also the header's own send button; the one under test is the
    // sheet's submit.
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Invia"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.sendTo[0]?.[0]).toBe("proj")
    );
  });

  it("126. the note typed in rides along with the send", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.change(
      screen.getByPlaceholderText("Ecco i file di cui parlavamo."),
      { target: { value: "eccolo" } }
    );
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Invia"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.sendTo[0]?.[2]).toBe("eccolo")
    );
  });

  it("127. a refused send says why instead of closing silently", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    harness.fail = new Set(["sendTo"]);
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Invia"
      )
    );
    await waitFor(() => expect(toastText()).toBeTruthy());
    // Still open, so the user can change something and retry.
    expect(useStore.getState().sheetPaths).not.toBeNull();
  });

  it("129. the Ticket mode mints a ticket and shows it", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("Ticket"));
    fireEvent.click(screen.getByText("Crea il ticket"));
    expect(await screen.findByText("arvc-test")).toBeTruthy();
  });

  it("130. the Link mode creates a link and shows it", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("Link"));
    fireEvent.click(screen.getByText("Crea il link"));
    expect(
      await screen.findByText("https://relay.test/dl/abc#key")
    ).toBeTruthy();
  });

  it("131. a relay that refuses links says so instead of showing nothing", async () => {
    harness.fail = new Set(["createLink"]);
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("Link"));
    fireEvent.click(screen.getByText("Crea il link"));
    await waitFor(() => expect(toastText()).toBeTruthy());
  });

  it("the Codice mode shows a code to read out", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("Codice"));
    fireEvent.click(screen.getByText("Genera il codice"));
    expect(await screen.findByText("4821-crater-mango")).toBeTruthy();
  });

  it("mailbox options only appear once the send is a deposit", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    await screen.findByText("proj");
    // TTL and password protect a *deposit*; offering them on a live send would
    // promise something that is not being applied.
    expect(screen.queryByText("Scade dopo")).toBeNull();
    fireEvent.click(screen.getByLabelText("Lascia in casella"));
    expect(await screen.findByText("Scade dopo")).toBeTruthy();
  });

  it("a deposit send carries its ttl, cap and password to the daemon", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Lascia in casella"));
    fireEvent.change(await screen.findByPlaceholderText("nessuna"), {
      target: { value: "segreto" },
    });
    fireEvent.click(screen.getByText("Deposita"));
    await waitFor(() => expect(harness.recorder.depositTo).toHaveLength(1));
    const [to, , ttl, max, password] = harness.recorder.depositTo[0];
    expect(to).toBe("proj");
    expect(ttl).toBe(7 * 24 * 3600);
    expect(max).toBe(1);
    expect(password).toBe("segreto");
  });

  it("a deposit hands back the arvm… ticket for hand-delivery", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Lascia in casella"));
    fireEvent.click(await screen.findByText("Deposita"));
    expect(await screen.findByText("arvm-test")).toBeTruthy();
  });

  it("reopening the panel never inherits the last send's password", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Lascia in casella"));
    fireEvent.change(await screen.findByPlaceholderText("nessuna"), {
      target: { value: "segreto" },
    });
    useStore.getState().closeSheet();
    useStore.getState().openSheet(["/b.txt"]);
    fireEvent.click(await screen.findByText("proj"));
    fireEvent.click(screen.getByLabelText("Lascia in casella"));
    // A password carried over would silently protect a different file for a
    // different person.
    expect(
      (
        (await screen.findByPlaceholderText("nessuna")) as HTMLInputElement
      ).value
    ).toBe("");
  });

  it("132. the ✕ closes the panel without sending", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(await screen.findByLabelText("Chiudi"));
    expect(useStore.getState().sheetPaths).toBeNull();
    expect(harness.recorder.sendTo).toEqual([]);
  });
});

// ---------------------------------------------------------------------------

describe("receiving from a pasted artefact", () => {
  it("recognises a pairing code and sends it to the daemon", async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("Ricevi…"));
    fireEvent.change(await screen.findByPlaceholderText("4821-crater-mango"), {
      target: { value: "4821-crater-mango" },
    });
    expect(
      screen.getByText(/Codice di accoppiamento/)
    ).toBeTruthy();
    fireEvent.click(
      within(document.querySelector(".sheet-foot") as HTMLElement).getByText(
        "Ricevi"
      )
    );
    await waitFor(() =>
      expect(harness.recorder.recv[0]?.[0]).toBe("4821-crater-mango")
    );
  });

  it("offers a password field only where a password can exist", async () => {
    render(<App />);
    fireEvent.click(await screen.findByText("Ricevi…"));
    const field = await screen.findByPlaceholderText("4821-crater-mango");
    fireEvent.change(field, { target: { value: "4821-crater-mango" } });
    // A pairing code has no password; suggesting otherwise teaches a falsehood.
    expect(screen.queryByLabelText(/Password/)).toBeNull();
    fireEvent.change(field, { target: { value: "arvm1234" } });
    await waitFor(() =>
      expect(screen.getByText(/Ticket di casella/)).toBeTruthy()
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
    expect(await screen.findByLabelText("Presenza sconosciuta")).toBeTruthy();
    expect(screen.queryByLabelText("Non collegato")).toBeNull();
  });

  it("a contact the relay reports as present is marked online", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", id: "p1" })];
    harness.snapshot.presence = { p1: true };
    render(<App />);
    useStore.getState().go("people");
    expect(await screen.findByLabelText("Online")).toBeTruthy();
  });

  it("Invia from a person card opens the sheet already addressed to them", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(
      within((await screen.findByText("proj")).closest(".person") as HTMLElement)
        .getByText("Invia")
    );
    // Choosing a person and then being asked to choose them again is the app
    // forgetting what it was just told.
    expect(useStore.getState().sheetTo).toBe("proj");
    // The recipient is already chosen, so the only thing still missing is the
    // payload — and the submit stays disabled until there is one.
    const submit = within(
      document.querySelector(".sheet-foot") as HTMLElement
    ).getByText("Invia").closest("button") as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    useStore.getState().openSheet(["/a.txt"], "proj");
    await waitFor(() => expect(submit.disabled).toBe(false));
    fireEvent.click(submit);
    await waitFor(() => expect(harness.recorder.sendTo[0]?.[0]).toBe("proj"));
  });

  it("117. a contact's fingerprint is reachable, and verifying is deliberate", async () => {
    harness.snapshot.contacts = [
      dto.contact({ name: "proj", verified: false }),
    ];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByText("Dettagli"));
    await screen.findByText("Impronta");
    const button = screen.getByText("Segna come verificato");
    // Reading the fingerprint and asserting you checked it are two acts.
    expect((button.closest("button") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(
      screen.getByLabelText(/Ho confrontato l'impronta/, { selector: "input" })
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
    fireEvent.click(await screen.findByLabelText("Azioni per proj"));
    fireEvent.click(
      within(await screen.findByRole("menu")).getByText("Scarica in automatico")
    );
    // Not sent yet: auto-downloading from an unconfirmed key is a MITM risk.
    expect(harness.recorder.markTrusted).toEqual([]);
    fireEvent.click(await screen.findByText("Forza comunque"));
    await waitFor(() =>
      expect(harness.recorder.markTrusted).toEqual([["proj", true]])
    );
  });

  it("a verified contact is trusted without a warning", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj", verified: true })];
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByLabelText("Azioni per proj"));
    fireEvent.click(
      within(await screen.findByRole("menu")).getByText("Scarica in automatico")
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
    fireEvent.click((await screen.findAllByText("Scambia contatti"))[0]);
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

  it("closing the sheet cancels the session on the daemon", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click((await screen.findAllByText("Scambia contatti"))[0]);
    await waitFor(() => expect(useStore.getState().pairing?.session).toBe("pair-1"));
    fireEvent.click(screen.getByLabelText("Chiudi"));
    await waitFor(() =>
      expect(harness.recorder.cancelPairing).toEqual(["pair-1"])
    );
  });

  it("a cancelled exchange is not reported back as an error", async () => {
    render(<App />);
    useStore.getState().go("people");
    fireEvent.click(await screen.findByText("Ho un codice"));
    await screen.findByText("Scambia i contatti");
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
    expect(await screen.findByText("Annullato.")).toBeTruthy();
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
      summary: "Collegato.",
      needs_restart: true,
    });
    expect(await screen.findByText("Riavvia e chiudi")).toBeTruthy();
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
      "Nome che mostri"
    )) as HTMLInputElement;
    fireEvent.change(field, { target: { value: "" } });
    fireEvent.click(
      within(field.closest(".hstack-sm") as HTMLElement).getByText("Salva")
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
    useStore.setState({ actionError: "qualcosa è andato storto" });
    fireEvent.click(await screen.findByLabelText("Chiudi"));
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

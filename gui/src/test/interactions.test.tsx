// Clicks. Not the store behind them — the actual controls, driven the way a user
// drives them.
//
// This layer was missing, and it is exactly where the "Accetta does nothing" bug
// lived. The store tests called `await store.accept(...)`; the button calls
// `store.accept(...)` with no await and no catch, from an `onClick` that cannot
// await anything. A refusal therefore vanished into the event loop — invisible to
// a test that awaited the promise itself, and to the user. Testing the function is
// not testing the button.
//
// So: press every control, and assert what the user would see happen.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
const dialogOpen = vi.fn(() => Promise.resolve(null as unknown));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: (...a: unknown[]) => dialogOpen(...(a as [])) }));
const revealItemInDir = vi.fn(() => Promise.resolve());
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (...a: unknown[]) => revealItemInDir(...(a as [])),
}));

import { useStore } from "../store";
import { App } from "../App";
import { Board } from "../components/Board";
import { SendSheet } from "../components/SendSheet";
import { IncomingModal } from "../components/IncomingModal";

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
    },
    guiVersion: "0.9.2",
    loadError: null,
    actionError: null,
    contacts: [],
    contactsById: {},
    transfers: {},
    search: "",
    pauseAll: false,
    openMenuKey: null,
    sheetPaths: null,
    incomingOfferId: null,
    deposits: [],
    depositsOpen: false,
    depositsLoading: false,
    depositsError: null,
    revoking: [],
  });
}

/** Seed the daemon's snapshot with one pending offer and mount the whole app,
 *  waiting for `init()`'s reload to land. Setting the store directly is not enough
 *  here: mounting `App` refetches, and would wipe a hand-placed row. */
async function renderAppWithOffer() {
  harness.snapshot.pending = [dto.offer({ id: "o1", from: "peer1", name: "arrivo.zip" })];
  render(<App />);
  await screen.findByText("arrivo.zip");
}

/** Same, for a send that is under way. */
async function renderAppWithSend(over: Partial<Record<string, unknown>> = {}) {
  harness.snapshot.transfers = [
    dto.transfer({ id: 1, name: "invio.txt", status: "active", ...(over as object) }),
  ];
  render(<App />);
  await screen.findByText("invio.txt");
}

/** An offer row exactly as the store builds one. */
function offerRow(over: Record<string, unknown> = {}) {
  return {
    key: "oo1",
    id: 0,
    offerId: "o1",
    dir: "in" as const,
    name: "arrivo.zip",
    size: 100,
    transferred: 0,
    status: "in arrivo" as const,
    peer: "proj",
    peerId: "peer1",
    encrypted: true,
    verified: false,
    method: "cloud" as const,
    swarmPeers: 0,
    downloadPeers: 0,
    files: 1,
    firstSeen: Date.now(),
    rank: 1,
    ...over,
  };
}
function sendRow(over: Record<string, unknown> = {}) {
  return { ...offerRow(), key: "t1", id: 1, offerId: undefined, dir: "out" as const, name: "invio.txt", status: "in corso" as const, ...over };
}

beforeEach(() => {
  resetHarness();
  fresh();
  dialogOpen.mockClear();
  revealItemInDir.mockClear();
});
afterEach(cleanup);

describe("the Accetta button", () => {
  it("102. pressing it tells the daemon to accept", async () => {
    useStore.setState({ transfers: { oo1: offerRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(harness.recorder.accept).toEqual([["o1", null]]));
  });

  it("103. when the daemon refuses, the user is told — not left guessing", async () => {
    // THE bug: the click fires a promise nobody awaits, so the refusal used to
    // disappear and the button looked inert. This is the test that was missing.
    harness.fail = new Set(["acceptOffer"]);
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(useStore.getState().actionError).toMatch(/accettare/i));
    expect(await screen.findByRole("alert")).toBeDefined();
  });

  it("104. a refused accept keeps the row, so it can be retried", async () => {
    useStore.setState({ transfers: { oo1: offerRow() as never } });
    harness.fail = new Set(["acceptOffer"]);
    render(<Board />);
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(useStore.getState().actionError).toBeTruthy());
    expect(useStore.getState().transfers.oo1).toBeTruthy();
  });

  it("105. it does not also open the details modal underneath it", async () => {
    // The button sits inside a row whose own click opens the modal.
    useStore.setState({ transfers: { oo1: offerRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(harness.recorder.accept).toHaveLength(1));
    expect(useStore.getState().incomingOfferId).toBeNull();
  });
});

describe("the Rifiuta button", () => {
  it("106. pressing it tells the daemon to reject", async () => {
    useStore.setState({ transfers: { oo1: offerRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByText("Rifiuta"));
    await waitFor(() => expect(harness.recorder.reject).toEqual(["o1"]));
  });

  it("107. a refused reject reports itself and keeps the row", async () => {
    harness.fail = new Set(["rejectOffer"]);
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("Rifiuta"));
    await waitFor(() => expect(useStore.getState().actionError).toMatch(/rifiutare/i));
    expect(useStore.getState().transfers.oo1, "a refused reject must keep the offer").toBeTruthy();
  });
});

describe("clicking a row", () => {
  it("108. an arrival opens its details", () => {
    useStore.setState({ transfers: { oo1: offerRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByText("arrivo.zip"));
    expect(useStore.getState().incomingOfferId).toBe("o1");
  });

  it("109. an ordinary transfer does not — there is nothing to confirm", () => {
    useStore.setState({ transfers: { t1: sendRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByText("invio.txt"));
    expect(useStore.getState().incomingOfferId).toBeNull();
  });
});

describe("the row menu", () => {
  it("110. the ⋮ opens it, and its actions reach the daemon", async () => {
    useStore.setState({ transfers: { t1: sendRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Metti in pausa"));
    await waitFor(() => expect(harness.recorder.pause).toEqual([1]));
  });

  it("111. a refused menu action reports itself", async () => {
    harness.fail = new Set(["pause"]);
    await renderAppWithSend();
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Metti in pausa"));
    await waitFor(() => expect(useStore.getState().actionError).toMatch(/pausa/i));
  });

  it("112. Riprendi resumes a paused transfer", async () => {
    useStore.setState({ transfers: { t1: sendRow({ status: "in attesa" }) as never } });
    render(<Board />);
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Riprendi"));
    await waitFor(() => expect(harness.recorder.resume).toEqual([1]));
  });

  it("113. Annulla invio cancels a live send", async () => {
    useStore.setState({ transfers: { t1: sendRow() as never } });
    render(<Board />);
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Annulla invio"));
    await waitFor(() => expect(harness.recorder.cancel).toEqual([1]));
  });

  it("114. Elimina removes a concluded one", async () => {
    useStore.setState({ transfers: { t1: sendRow({ status: "completato" }) as never } });
    render(<Board />);
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Elimina"));
    await waitFor(() => expect(harness.recorder.remove).toEqual([1]));
  });

  it("115. a refused Elimina keeps the row on screen", async () => {
    harness.fail = new Set(["remove"]);
    await renderAppWithSend({ status: "completed" });
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Elimina"));
    await waitFor(() => expect(useStore.getState().actionError).toMatch(/eliminare/i));
    expect(useStore.getState().transfers.t1, "a refused remove must keep the row").toBeTruthy();
  });

  it("116. Apri cartella reveals a completed download", async () => {
    useStore.setState({
      transfers: { t1: sendRow({ status: "completato", path: "/Users/ls/Arvolo/x" }) as never },
    });
    render(<Board />);
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Apri cartella"));
    await waitFor(() => expect(revealItemInDir).toHaveBeenCalledWith("/Users/ls/Arvolo/x"));
  });

  it("117. Verifica identità is offered for a saved contact and marks it", async () => {
    useStore.setState({
      contacts: [dto.contact({ name: "proj", id: "peer1" })],
      contactsById: { peer1: dto.contact({ name: "proj", id: "peer1" }) },
      transfers: { t1: sendRow({ verified: false }) as never },
    });
    render(<Board />);
    fireEvent.click(screen.getByLabelText("Azioni trasferimento"));
    fireEvent.click(screen.getByText("Verifica identità"));
    await waitFor(() => expect(harness.recorder.markVerified).toEqual(["proj"]));
  });

  it("118. Sposta giù reorders without touching the daemon", async () => {
    useStore.setState({
      transfers: {
        t1: sendRow({ key: "t1", id: 1, name: "primo", rank: 2 }) as never,
        t2: sendRow({ key: "t2", id: 2, name: "secondo", rank: 1 }) as never,
      },
    });
    render(<Board />);
    fireEvent.click(screen.getAllByLabelText("Azioni trasferimento")[0]);
    fireEvent.click(screen.getByText("Sposta giù"));
    expect(useStore.getState().transfers.t1.rank).toBe(1);
    expect(harness.recorder.cancel).toEqual([]); // nothing was sent anywhere
  });
});

describe("the header", () => {
  it("119. the bell opens the oldest arrival", async () => {
    await renderAppWithOffer();
    fireEvent.click(screen.getByTitle(/1 file in arrivo/i));
    expect(useStore.getState().incomingOfferId).toBe("o1");
  });

  it("120. with nothing waiting, the bell does nothing", () => {
    render(<App />);
    fireEvent.click(screen.getByTitle(/nessun arrivo/i));
    expect(useStore.getState().incomingOfferId).toBeNull();
  });

  it("121. the code chip copies the id", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    render(<App />);
    fireEvent.click(screen.getByTitle("Copia il tuo codice"));
    expect(writeText).toHaveBeenCalledWith(
      "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha"
    );
    expect(await screen.findByText(/copiato/i)).toBeDefined();
  });
});

describe("the control row", () => {
  it("122. typing in the search filters the board", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "relazione.pdf" }),
      dto.transfer({ id: 2, name: "foto.zip" }),
    ];
    render(<App />);
    await screen.findByText("relazione.pdf");
    fireEvent.change(screen.getByPlaceholderText(/cerca file/i), {
      target: { value: "relaz" },
    });
    expect(screen.getByText("relazione.pdf")).toBeDefined();
    expect(screen.queryByText("foto.zip")).toBeNull();
  });

  it("123. Pausa tutto pauses what is running, and the label flips", async () => {
    await renderAppWithSend();
    fireEvent.click(screen.getByText("Pausa tutto"));
    await waitFor(() => expect(harness.recorder.pause).toEqual([1]));
    expect(await screen.findByText("Riprendi tutto")).toBeDefined();
  });

  it("124. Scegli file opens the OS picker", async () => {
    render(<App />);
    fireEvent.click(screen.getByText("Scegli file"));
    await waitFor(() => expect(dialogOpen).toHaveBeenCalled());
  });
});

describe("the send panel", () => {
  it("125. clicking a contact sends to them and closes the panel", async () => {
    const c = dto.contact({ name: "proj", id: "peer1" });
    useStore.setState({ contacts: [c], contactsById: { peer1: c }, sheetPaths: ["/a.txt"] });
    render(<SendSheet />);
    fireEvent.click(screen.getByText("proj"));
    await waitFor(() =>
      expect(harness.recorder.sendTo).toEqual([["proj", ["/a.txt"], ""]])
    );
    expect(useStore.getState().sheetPaths).toBeNull();
  });

  it("126. the note typed in rides along with the send", async () => {
    const c = dto.contact({ name: "proj", id: "peer1" });
    useStore.setState({ contacts: [c], contactsById: { peer1: c }, sheetPaths: ["/a.txt"] });
    render(<SendSheet />);
    fireEvent.change(screen.getByPlaceholderText(/messaggio/i), {
      target: { value: "ecco i file" },
    });
    fireEvent.click(screen.getByText("proj"));
    await waitFor(() =>
      expect(harness.recorder.sendTo).toEqual([["proj", ["/a.txt"], "ecco i file"]])
    );
  });

  it("127. a refused send shows why, inside the panel", async () => {
    const c = dto.contact({ name: "proj", id: "peer1" });
    useStore.setState({ contacts: [c], contactsById: { peer1: c }, sheetPaths: ["/a.txt"] });
    harness.fail = new Set(["sendTo"]);
    render(<SendSheet />);
    fireEvent.click(screen.getByText("proj"));
    expect(await screen.findByText(/daemon refused/i)).toBeDefined();
    // The panel stays open so the user can try another route.
    expect(useStore.getState().sheetPaths).not.toBeNull();
  });

  it("128. the ID tab sends to a pasted code", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<SendSheet />);
    fireEvent.click(screen.getByText("ID / QR"));
    fireEvent.change(screen.getByPlaceholderText(/if2xmne/i), {
      target: { value: "abc123" },
    });
    fireEvent.click(screen.getByText("Invia a questo ID"));
    await waitFor(() => expect(harness.recorder.sendTo).toEqual([["abc123", ["/a.txt"], ""]]));
  });

  it("129. the Ticket tab mints a ticket and shows it", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<SendSheet />);
    fireEvent.click(screen.getByText("Ticket"));
    fireEvent.click(screen.getByText("Genera ticket P2P"));
    expect(await screen.findByText("arvc-test")).toBeDefined();
  });

  it("130. the Link tab creates a link and shows it", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<SendSheet />);
    fireEvent.click(screen.getByText("Link"));
    fireEvent.click(screen.getByText("Crea link di download"));
    expect(
      await screen.findByDisplayValue("https://relay.test/dl/abc#key")
    ).toBeDefined();
  });

  it("131. a relay that refuses links says so instead of showing nothing", async () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    harness.fail = new Set(["createLink"]);
    render(<SendSheet />);
    fireEvent.click(screen.getByText("Link"));
    fireEvent.click(screen.getByText("Crea link di download"));
    expect(await screen.findByText(/daemon refused/i)).toBeDefined();
  });

  it("132. the ✕ closes the panel without sending", () => {
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<SendSheet />);
    fireEvent.click(screen.getByText("✕"));
    expect(useStore.getState().sheetPaths).toBeNull();
    expect(harness.recorder.sendTo).toEqual([]);
  });
});

describe("the incoming modal", () => {
  it("133. Accetta uses the folder the user picked", async () => {
    useStore.setState({ transfers: { oo1: offerRow() as never }, incomingOfferId: "o1" });
    dialogOpen.mockResolvedValueOnce("/tmp/scelta");
    render(<IncomingModal />);
    fireEvent.click(screen.getByText(/salva in/i));
    await waitFor(() => expect(dialogOpen).toHaveBeenCalled());
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(harness.recorder.accept).toEqual([["o1", "/tmp/scelta"]]));
  });

  it("134. Accetta without picking one uses the daemon's default", async () => {
    useStore.setState({ transfers: { oo1: offerRow() as never }, incomingOfferId: "o1" });
    render(<IncomingModal />);
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(harness.recorder.accept).toEqual([["o1", null]]));
  });

  it("135. it shows the sender's note", () => {
    useStore.setState({
      transfers: { oo1: offerRow({ note: "ecco le foto di ieri" }) as never },
      incomingOfferId: "o1",
    });
    render(<IncomingModal />);
    expect(screen.getByText("ecco le foto di ieri")).toBeDefined();
  });

  it("136. Rifiuta from the modal rejects", async () => {
    useStore.setState({ transfers: { oo1: offerRow() as never }, incomingOfferId: "o1" });
    render(<IncomingModal />);
    fireEvent.click(screen.getByText("Rifiuta"));
    await waitFor(() => expect(harness.recorder.reject).toEqual(["o1"]));
  });

  it("137. it names the real download folder, not a guess", () => {
    useStore.setState({ transfers: { oo1: offerRow() as never }, incomingOfferId: "o1" });
    render(<IncomingModal />);
    expect(screen.getByText(/Arvolo/)).toBeDefined();
  });
});

describe("no dead ends to the CLI", () => {
  it("156. the Link tab points at the panel, not at a command to type", async () => {
    // It used to say "Revocalo dalla CLI con `arvolo deposits`" — a command that
    // does not exist in the CLI at all. The advice was unfollowable twice over.
    useStore.setState({ sheetPaths: ["/a.txt"] });
    render(<App />);
    fireEvent.click(screen.getByText("Link"));
    fireEvent.click(screen.getByText("Crea link di download"));
    await screen.findByDisplayValue("https://relay.test/dl/abc#key");

    expect(screen.queryByText(/arvolo deposits/)).toBeNull();
    fireEvent.click(screen.getByText("Link e depositi"));
    await waitFor(() => expect(useStore.getState().depositsOpen).toBe(true));
    expect(useStore.getState().sheetPaths).toBeNull();
  });
});

describe("the error banner", () => {
  it("138. it can be dismissed", async () => {
    harness.fail = new Set(["acceptOffer"]);
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("Accetta"));
    const alert = await screen.findByRole("alert");
    fireEvent.click(within(alert).getByLabelText("Chiudi"));
    await waitFor(() => expect(useStore.getState().actionError).toBeNull());
  });

  it("139. a later success clears it without a click", async () => {
    harness.fail = new Set(["acceptOffer"]);
    await renderAppWithOffer();
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(useStore.getState().actionError).toBeTruthy());

    // The offer is still there precisely because the accept was refused: retry it.
    harness.fail = new Set();
    fireEvent.click(screen.getByText("Accetta"));
    await waitFor(() => expect(useStore.getState().actionError).toBeNull());
  });
});

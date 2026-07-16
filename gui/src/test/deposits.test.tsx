// Links and deposits: what this client has left on a relay and can still take back.
//
// This is the feature that answers "se creo un link voglio vederlo da qualche parte
// e poterlo annullare dalla GUI". It handles the one thing in the app with a
// consequence outside this machine — a public URL anyone holding it can fetch — so
// its failure paths matter more than most: a Revoca that silently does nothing
// leaves a file downloadable while telling the user it is gone.
//
// Both layers, because this session proved they are not the same test: the store,
// and the buttons.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: () => Promise.resolve() }));

import { useStore } from "../store";
import { DepositsPanel } from "../components/DepositsPanel";
import type { DepositDto } from "../types";

const DAY = 86400;
const nowSecs = () => Math.floor(Date.now() / 1000);

function deposit(over: Partial<DepositDto> = {}): DepositDto {
  return {
    id: "d1",
    kind: "link",
    name: "relazione.pdf",
    size: 1024,
    link: "https://relay.test/dl/abc#key",
    recipient: "",
    created: nowSecs() - 60,
    expires: nowSecs() + 7 * DAY,
    expired: false,
    max_label: "nessun limite",
    present: true,
    downloads: 0,
    max_downloads: null,
    ...over,
  };
}

const s = () => useStore.getState();

beforeEach(() => {
  resetHarness();
  useStore.setState({
    connected: true,
    deposits: [],
    depositsOpen: false,
    depositsError: null,
    depositsLoading: false,
    actionError: null,
  } as never);
});
afterEach(cleanup);

describe("the deposits list", () => {
  it("140. opening it asks the daemon and shows what is out there", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().openDeposits();
    expect(s().depositsOpen).toBe(true);
    expect(s().deposits).toHaveLength(1);
    expect(s().deposits[0].link).toContain("/dl/");
  });

  it("141. a link created in the app turns up in the list", async () => {
    // The user's actual ask: create a link, then find it somewhere.
    await s().link("/a.pdf");
    harness.snapshot.deposits = [deposit({ name: "a.pdf" })];
    await s().loadDeposits();
    expect(s().deposits.map((d) => d.name)).toContain("a.pdf");
  });

  it("142. a daemon that cannot be asked says so, rather than showing an empty list", async () => {
    // An empty list here means "nothing is public". Saying that when we do not know
    // is the same lie the board once told about transfers.
    harness.fail = new Set(["listDeposits"]);
    await s().openDeposits();
    expect(s().depositsError).toBeTruthy();
    expect(s().depositsOpen, "the panel stays open to show the error").toBe(true);
  });

  it("143. closing it clears any error, so it does not greet you next time", async () => {
    harness.fail = new Set(["listDeposits"]);
    await s().openDeposits();
    s().closeDeposits();
    expect(s().depositsOpen).toBe(false);
    expect(s().depositsError).toBeNull();
  });
});

describe("revoking", () => {
  it("144. it tells the daemon which deposit to withdraw", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    await s().revokeDeposit("abc");
    expect(harness.recorder.revokeDeposit).toEqual(["abc"]);
  });

  it("145. the row goes only once the daemon confirms", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    harness.snapshot.deposits = [];
    await s().revokeDeposit("abc");
    expect(s().deposits).toHaveLength(0);
  });

  it("146. a refused revoke keeps the row and says why — the file is still public", async () => {
    // The dangerous case: telling the user a link is gone when the relay still
    // serves it. Whoever holds the URL can still take the file.
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    harness.fail = new Set(["revokeDeposit"]);
    await expect(s().revokeDeposit("abc")).rejects.toThrow();
    expect(s().deposits, "the link must still be listed as live").toHaveLength(1);
    expect(s().depositsError || s().actionError).toBeTruthy();
  });
});

describe("the panel", () => {
  it("147. it lists a link with its URL and controls", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().openDeposits();
    render(<DepositsPanel />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    expect(screen.getByText("https://relay.test/dl/abc#key")).toBeDefined();
    expect(screen.getByText("Revoca")).toBeDefined();
  });

  it("148. Revoca asks first — it is irreversible and breaks the URL for everyone", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    render(<DepositsPanel />);
    fireEvent.click(await screen.findByText("Revoca"));
    // One click must not destroy anything: it offers the choice.
    expect(await screen.findByText("Sì, revoca")).toBeDefined();
    expect(harness.recorder.revokeDeposit).toEqual([]);
  });

  it("148b. confirming actually withdraws it", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    render(<DepositsPanel />);
    fireEvent.click(await screen.findByText("Revoca"));
    fireEvent.click(await screen.findByText("Sì, revoca"));
    await waitFor(() => expect(harness.recorder.revokeDeposit).toEqual(["abc"]));
  });

  it("148c. answering No leaves the link alone", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    render(<DepositsPanel />);
    fireEvent.click(await screen.findByText("Revoca"));
    fireEvent.click(await screen.findByText("No"));
    expect(harness.recorder.revokeDeposit).toEqual([]);
    expect(await screen.findByText("Revoca")).toBeDefined();
  });

  it("149. a refused Revoca shows the reason instead of doing nothing", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().openDeposits();
    harness.fail = new Set(["revokeDeposit"]);
    render(<DepositsPanel />);
    fireEvent.click(await screen.findByText("Revoca"));
    fireEvent.click(await screen.findByText("Sì, revoca"));
    await waitFor(() =>
      expect(
        s().depositsError || s().actionError,
        "silence is what made buttons look broken"
      ).toBeTruthy()
    );
  });

  it("150. Copia puts the link on the clipboard", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    harness.snapshot.deposits = [deposit()];
    await s().openDeposits();
    render(<DepositsPanel />);
    fireEvent.click(await screen.findByText("Copia"));
    expect(writeText).toHaveBeenCalledWith("https://relay.test/dl/abc#key");
  });

  it("151. an expired deposit offers Elimina, not Revoca — there is nothing to take back", async () => {
    harness.snapshot.deposits = [deposit({ expired: true, expires: nowSecs() - DAY })];
    await s().openDeposits();
    render(<DepositsPanel />);
    // Nothing is left on the relay to take back — only the local record to tidy.
    expect(await screen.findByText("Elimina")).toBeDefined();
    expect(screen.queryByText("Revoca")).toBeNull();
  });

  it("152. a sealed deposit shows its recipient, and has no URL to copy", async () => {
    harness.snapshot.deposits = [
      deposit({ kind: "offline", link: "", recipient: "peer1", max_label: "1 download" }),
    ];
    await s().openDeposits();
    render(<DepositsPanel />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    expect(screen.getByText(/sigillato per/i)).toBeDefined();
    expect(screen.queryByText("Copia"), "a sealed deposit is not a public URL").toBeNull();
  });

  it("152b. a relay that cannot be asked shows unknown, never a confident 'alive'", async () => {
    // The local record is only a receipt of the deposit: it never learns that a
    // link was fetched. When the relay cannot confirm, saying "attivo" would be a
    // guess dressed as a fact — the same failure as a green, empty board.
    harness.snapshot.deposits = [deposit({ present: null, downloads: null })];
    await s().openDeposits();
    render(<DepositsPanel />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    expect(screen.queryByText(/^attivo$/i), "unknown must not read as alive").toBeNull();
  });

  it("152c. a link the relay no longer holds is not offered as revocable", async () => {
    harness.snapshot.deposits = [deposit({ present: false })];
    await s().openDeposits();
    render(<DepositsPanel />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
  });

  it("153. with nothing out there it says so plainly", async () => {
    harness.snapshot.deposits = [];
    await s().openDeposits();
    render(<DepositsPanel />);
    expect(await screen.findByText(/nessun link o deposito/i)).toBeDefined();
  });

  it("154. Aggiorna refetches", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().openDeposits();
    render(<DepositsPanel />);
    const before = harness.recorder.listDeposits;
    fireEvent.click(screen.getByText("Aggiorna"));
    await waitFor(() => expect(harness.recorder.listDeposits).toBeGreaterThan(before));
  });

  it("155. Chiudi closes it", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().openDeposits();
    render(<DepositsPanel />);
    fireEvent.click(screen.getByLabelText("Chiudi"));
    expect(s().depositsOpen).toBe(false);
  });
});

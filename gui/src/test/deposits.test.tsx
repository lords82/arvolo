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
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: () => Promise.resolve(),
  openUrl: () => Promise.resolve(),
  openPath: () => Promise.resolve(),
}));

import { useStore } from "../store";
import { App } from "../App";
import { DepositsView } from "../views/DepositsView";
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
    route: "deposits",
    deposits: [],
    depositsError: null,
    depositsLoading: false,
    actionError: null,
  } as never);
});
afterEach(cleanup);

describe("the deposits list", () => {
  it("140. opening it asks the daemon and shows what is out there", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().go("deposits");
    expect(s().route).toBe("deposits");
    expect(s().deposits).toHaveLength(1);
    expect(s().deposits[0].link).toContain("/dl/");
  });

  it("141. a link created in the app turns up in the list", async () => {
    // The user's actual ask: create a link, then find it somewhere.
    await s().link("/a.pdf", null, null);
    harness.snapshot.deposits = [deposit({ name: "a.pdf" })];
    await s().loadDeposits();
    expect(s().deposits.map((d) => d.name)).toContain("a.pdf");
  });

  it("142. a daemon that cannot be asked says so, rather than showing an empty list", async () => {
    // An empty list here means "nothing is public". Saying that when we do not know
    // is the same lie the board once told about transfers.
    harness.fail = new Set(["listDeposits"]);
    await s().go("deposits");
    expect(s().depositsError).toBeTruthy();
    expect(s().route, "the screen stays put to show the error").toBe("deposits");
  });

  it("143. closing it clears any error, so it does not greet you next time", async () => {
    harness.fail = new Set(["listDeposits"]);
    await s().go("deposits");
    s().go("transfers");
    expect(s().route).toBe("transfers");
    expect(s().depositsError).toBeNull();
  });
});

describe("revoking", () => {
  it("144. it tells the daemon which deposit to withdraw", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    await s().revokeDeposit("abc");
    expect(harness.recorder.revokeDeposit).toEqual(["abc"]);
  });

  it("145. the row goes only once the daemon confirms", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    harness.snapshot.deposits = [];
    await s().revokeDeposit("abc");
    expect(s().deposits).toHaveLength(0);
  });

  it("146. a refused revoke keeps the row and says why — the file is still public", async () => {
    // The dangerous case: telling the user a link is gone when the relay still
    // serves it. Whoever holds the URL can still take the file.
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    harness.fail = new Set(["revokeDeposit"]);
    await expect(s().revokeDeposit("abc")).rejects.toThrow();
    expect(s().deposits, "the link must still be listed as live").toHaveLength(1);
    expect(s().depositsError || s().actionError).toBeTruthy();
  });
});

describe("the panel", () => {
  it("147. it lists a link with its URL and controls", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    expect(screen.getByText("https://relay.test/dl/abc#key")).toBeDefined();
    expect(screen.getByText("Revoca", { selector: ".row button" })).toBeDefined();
  });

  it("148. Revoca asks first — it is irreversible and breaks the URL for everyone", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    render(<DepositsView />);
    fireEvent.click(await screen.findByText("Revoca", { selector: ".row button" }));
    // One click must not destroy anything: it offers the choice.
    expect(await screen.findByText("Revoca", { selector: ".sheet-foot button" })).toBeDefined();
    expect(harness.recorder.revokeDeposit).toEqual([]);
  });

  it("148b. confirming actually withdraws it", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    render(<DepositsView />);
    fireEvent.click(await screen.findByText("Revoca", { selector: ".row button" }));
    fireEvent.click(await screen.findByText("Revoca", { selector: ".sheet-foot button" }));
    await waitFor(() => expect(harness.recorder.revokeDeposit).toEqual(["abc"]));
  });

  it("148c. answering No leaves the link alone", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    render(<DepositsView />);
    fireEvent.click(await screen.findByText("Revoca", { selector: ".row button" }));
    fireEvent.click(await screen.findByText("Annulla"));
    expect(harness.recorder.revokeDeposit).toEqual([]);
    expect(await screen.findByText("Revoca", { selector: ".row button" })).toBeDefined();
  });

  it("149. a refused Revoca shows the reason instead of doing nothing", async () => {
    harness.snapshot.deposits = [deposit({ id: "abc" })];
    await s().go("deposits");
    harness.fail = new Set(["revokeDeposit"]);
    render(<DepositsView />);
    fireEvent.click(await screen.findByText("Revoca", { selector: ".row button" }));
    fireEvent.click(await screen.findByText("Revoca", { selector: ".sheet-foot button" }));
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
    await s().go("deposits");
    render(<DepositsView />);
    fireEvent.click(await screen.findByLabelText("Copia"));
    expect(writeText).toHaveBeenCalledWith("https://relay.test/dl/abc#key");
  });

  it("151. an expired deposit offers Elimina, not Revoca — there is nothing to take back", async () => {
    harness.snapshot.deposits = [deposit({ expired: true, expires: nowSecs() - DAY })];
    await s().go("deposits");
    render(<DepositsView />);
    // Nothing is left on the relay to take back — only the local record to tidy.
    expect(await screen.findByText("Rimuovi")).toBeDefined();
    expect(screen.queryByText("Revoca", { selector: ".row button" })).toBeNull();
  });

  it("152. a sealed deposit shows its recipient, and has no URL to copy", async () => {
    harness.snapshot.deposits = [
      deposit({ kind: "offline", link: "", recipient: "peer1", max_label: "1 download" }),
    ];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    expect(screen.getByText(/sigillato per/i)).toBeDefined();
    expect(screen.queryByLabelText("Copia"), "a sealed deposit is not a public URL").toBeNull();
  });

  it("152b. a relay that cannot be asked shows unknown, never a confident 'alive'", async () => {
    // The local record is only a receipt of the deposit: it never learns that a
    // link was fetched. When the relay cannot confirm, saying "attivo" would be a
    // guess dressed as a fact — the same failure as a green, empty board.
    harness.snapshot.deposits = [deposit({ present: null, downloads: null })];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    expect(screen.queryByText(/^attivo$/i), "unknown must not read as alive").toBeNull();
  });

  it("152c. a link the relay no longer holds is not offered as revocable", async () => {
    harness.snapshot.deposits = [deposit({ present: false })];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText("relazione.pdf")).toBeDefined();
    // The claim in this test's name, actually asserted: the relay has let it go, so
    // there is nothing to withdraw — only the local record to tidy away. Offering
    // "Revoca" would promise an action that cannot happen.
    expect(screen.getByText("Non più disponibile")).toBeDefined();
    expect(screen.getByText("Rimuovi")).toBeDefined();
    expect(screen.queryByText("Revoca", { selector: ".row button" })).toBeNull();
  });

  it("153. with nothing out there it says so plainly", async () => {
    harness.snapshot.deposits = [];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText(/nessun link o deposito/i)).toBeDefined();
  });

  it("154. Aggiorna refetches", async () => {
    harness.snapshot.deposits = [deposit()];
    await s().go("deposits");
    render(<DepositsView />);
    const before = harness.recorder.listDeposits;
    fireEvent.click(screen.getByText("Aggiorna"));
    await waitFor(() => expect(harness.recorder.listDeposits).toBeGreaterThan(before));
  });

  it("155. Aggiorna re-asks the relay rather than trusting a cached list", async () => {
    // The screen is a place now, not a panel with a close button — leaving it is
    // navigating away. What still matters is that the list can be re-read on
    // demand: nothing pushes an update when a link is fetched.
    harness.snapshot.deposits = [deposit()];
    await s().go("deposits");
    render(<DepositsView />);
    const before = harness.recorder.listDeposits;
    fireEvent.click(screen.getByText("Aggiorna"));
    await waitFor(() =>
      expect(harness.recorder.listDeposits).toBe(before + 1)
    );
  });

  it("157. a live link says how many times it was actually fetched", async () => {
    // Not the cap that was asked for at deposit time — the count the relay reports.
    // The local record cannot know this: it is written once and never updated.
    harness.snapshot.deposits = [deposit({ present: true, downloads: 3, max_downloads: 5 })];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText("Attivo")).toBeDefined();
    expect(screen.getByText(/3\/5 download/)).toBeDefined();
  });

  it("158. with nothing out there, it still does not send the user to a terminal", async () => {
    harness.snapshot.deposits = [];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText(/nessun link o deposito/i)).toBeDefined();
    expect(screen.queryByText(/CLI|terminale|arvolo /i)).toBeNull();
  });

  it("159. a sealed deposit calls its recipient by name", async () => {
    // Sealed deposits only started reaching this panel once the daemon began
    // filing a record for the mailbox sends it makes — before that, this whole
    // branch rendered for nobody. The board names people; a raw key here would
    // be the one place that doesn't.
    useStore.setState({
      contactsById: {
        k7x2: { id: "k7x2", name: "alice", verified: true, fingerprint: "aa bb", trusted: false, blocked: false, display_name: "", pending_name: "" },
      },
    });
    harness.snapshot.deposits = [
      deposit({ kind: "offline", link: "", recipient: "k7x2", max_label: "1" }),
    ];
    await s().go("deposits");
    render(<DepositsView />);
    expect(await screen.findByText(/sigillato per alice/)).toBeDefined();
  });

  it("160. a sealed deposit for someone not in the book still says who", async () => {
    // No name to show, but the id is not nothing — it must not render "sigillato
    // per " and trail off.
    useStore.setState({ contactsById: {} });
    harness.snapshot.deposits = [
      deposit({ kind: "offline", link: "", recipient: "zz99aabbccdd", max_label: "1" }),
    ];
    await s().go("deposits");
    render(<DepositsView />);
    const line = await screen.findByText(/sigillato per \S+/);
    expect(line.textContent).not.toMatch(/sigillato per\s*$/);
  });
});

describe("reaching the panel from the app", () => {
  it("159. the sidebar entry opens it and fetches what is out there", async () => {
    harness.snapshot.deposits = [deposit({ name: "vacanze.zip" })];
    useStore.setState({ route: "transfers" });
    render(<App />);
    fireEvent.click(
      within(screen.getByRole("navigation")).getByText("Link e depositi")
    );
    expect(await screen.findByText("vacanze.zip")).toBeDefined();
    expect(harness.recorder.listDeposits).toBeGreaterThan(0);
  });

  it("160. opening always refetches — nothing pushes this list", async () => {
    // No engine event exists for a deposit, and a relay never reports a download
    // back, so what is in the store is only ever as fresh as the last fetch. If
    // opening trusted the cache, the panel would quietly show yesterday's truth.
    useStore.setState({
      route: "transfers",
      deposits: [deposit({ name: "vecchio.zip" })],
    });
    harness.snapshot.deposits = [deposit({ name: "nuovo.zip" })];
    render(<App />);
    fireEvent.click(
      within(screen.getByRole("navigation")).getByText("Link e depositi")
    );
    expect(await screen.findByText("nuovo.zip")).toBeDefined();
    expect(screen.queryByText("vecchio.zip")).toBeNull();
  });
});

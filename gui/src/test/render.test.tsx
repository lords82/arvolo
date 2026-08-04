// Rendering, not just state. A thrown render unmounts the whole React tree and
// leaves an empty window — the worst failure this app has, because it looks like a
// freeze and says nothing. Dropping a file is the one gesture that mounts the send
// panel, so that is where it has to be proven.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());

// The Tauri APIs the components reach for. In a test there is no webview.
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: () => Promise.resolve(() => {}),
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: () => Promise.resolve(),
}));

import { useStore } from "../store";
import { App } from "../App";
import { SendSheet } from "../components/SendSheet";
import { Board } from "../components/Board";
import { ErrorBoundary } from "../ErrorBoundary";

function reset() {
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
    contacts: [],
    contactsById: {},
    transfers: {},
    search: "",
    pauseAll: false,
    openMenuKey: null,
    sheetPaths: null,
    incomingOfferId: null,
  });
}

beforeEach(() => {
  resetHarness();
  reset();
});
afterEach(cleanup);

describe("the window survives what the user does to it", () => {
  it("48. the panel survives going from closed to open — the drop itself", async () => {
    // The one that actually bit: rendering the sheet with paths already set is not
    // the gesture. The gesture is a *transition* — it renders closed (returns null
    // early), then a drop gives it paths and it renders open. A hook declared below
    // that early return only runs on the second pass, so React sees the hook count
    // grow mid-life and tears the whole tree down (#310) — a blank window.
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <ErrorBoundary>
        <SendSheet />
      </ErrorBoundary>
    );
    expect(screen.queryByText("Invia")).toBeNull(); // closed

    await act(async () => {
      useStore.getState().openSheet(["/Users/ls/Scrivania/relazione.pdf"]);
    });

    expect(screen.queryByText(/si è rotto/i), "the tree must not crash").toBeNull();
    expect(screen.getByText("relazione.pdf")).toBeDefined();
    spy.mockRestore();
  });

  it("39. dropping a file opens the send panel without blanking the window", () => {
    // The exact gesture: a real OS drop hands the store a path, which mounts the
    // send sheet. If that render throws, the tree unmounts and the window goes white.
    useStore.getState().openSheet(["/Users/ls/Scrivania/relazione.pdf"]);
    render(<SendSheet />);
    expect(screen.getByText("Invia")).toBeDefined();
    expect(screen.getByText("relazione.pdf")).toBeDefined();
  });

  it("40. dropping several files at once still renders", () => {
    useStore.getState().openSheet(["/a/one.txt", "/a/two.txt", "/a/three.txt"]);
    render(<SendSheet />);
    expect(screen.getByText("3 file")).toBeDefined();
  });

  it("41. a path with no extension, spaces or unicode does not break the chip", () => {
    useStore.getState().openSheet(["/Users/ls/Scrivania/Relazione finale — 2026"]);
    render(<SendSheet />);
    expect(screen.getByText("Relazione finale — 2026")).toBeDefined();
  });

  it("42. the full app mounts with an empty board", () => {
    render(<App />);
    expect(screen.getByText("Inviati")).toBeDefined();
    expect(screen.getByText("Ricevuti")).toBeDefined();
  });

  it("43. the board renders a row of every status without throwing", () => {
    const statuses = [
      "in corso",
      "in attesa",
      "in stallo",
      "in annullamento",
      "deposited",
      "completato",
      "fallito",
      "annullato",
    ] as const;
    const transfers: Record<string, any> = {};
    statuses.forEach((status, i) => {
      transfers[`t${i}`] = {
        key: `t${i}`,
        id: i,
        dir: i % 2 ? "out" : "in",
        name: `f${i}.bin`,
        size: 100,
        transferred: 50,
        status,
        reason: "una ragione molto lunga che potrebbe rompere il layout",
        encrypted: true,
        verified: i % 3 === 0,
        method: "p2p",
        swarmPeers: i,
        downloadPeers: i,
        files: 1,
        firstSeen: Date.now(),
        rank: i,
      };
    });
    useStore.setState({ transfers });
    render(<Board />);
    expect(screen.getByText("f0.bin")).toBeDefined();
  });

  it("44. an incoming offer row renders with its accept/reject buttons", () => {
    useStore.setState({
      transfers: {
        oo1: {
          key: "oo1",
          id: 0,
          offerId: "o1",
          dir: "in",
          name: "arrivo.zip",
          size: 10,
          transferred: 0,
          status: "in arrivo",
          encrypted: true,
          verified: false,
          method: "cloud",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: Date.now(),
          rank: 1,
        },
      },
    });
    render(<Board />);
    expect(screen.getByText("Accetta")).toBeDefined();
    expect(screen.getByText("Rifiuta")).toBeDefined();
  });

  it("45. the send panel lists saved contacts to send to", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    useStore.setState({ contacts: [dto.contact({ name: "proj" })] });
    useStore.getState().openSheet(["/a.txt"]);
    render(<SendSheet />);
    expect(screen.getByText("proj")).toBeDefined();
  });

  it("47. a file released anywhere in the window is accepted, not navigated to", () => {
    // Releasing a file outside the dashed strip used to hit the webview's default —
    // navigate to it — which replaced the app with a blank page. Nothing in the
    // document may accept that default any more.
    const drop = new Event("drop", { cancelable: true, bubbles: true });
    const over = new Event("dragover", { cancelable: true, bubbles: true });
    render(<App />);

    // Land them on the board, far from the drop zone.
    document.body.dispatchEvent(over);
    document.body.dispatchEvent(drop);

    expect(over.defaultPrevented, "dragover must be refused").toBe(true);
    expect(drop.defaultPrevented, "drop must be refused").toBe(true);
  });

  it("46. a crash shows what broke instead of an empty window", () => {
    // The blank window is the failure we cannot debug: it says nothing, and a
    // release build has no devtools to ask. Whatever throws, the user must at least
    // see it — and be told their transfers are still running in the daemon.
    const Boom = () => {
      throw new Error("boom: qualcosa è andato storto");
    };
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>
    );
    expect(screen.getByText(/si è rotto/i)).toBeDefined();
    expect(screen.getByText(/boom: qualcosa è andato storto/)).toBeDefined();
    expect(screen.getByText("Riprova")).toBeDefined();
    spy.mockRestore();
  });
});

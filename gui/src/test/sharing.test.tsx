// A share is not a transfer, and the board has to be able to tell you so.
//
// This is the bug these tests were written for: after downloading one file the
// user had two outgoing rows for it and neither made sense. One was the ticket
// that had served it, pinned at 100% for ever and reading as stuck; the other was
// the seeding the finished download had turned into, sitting at 0% and reading as
// a send of a file they never sent. Both were "active", because the engine calls
// anything it is running active — true, and no help at all.
//
// So: a row that exists to be fetched says that, and what it has actually done
// lives one click away, where there is room to say it honestly.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));
const openPathMock = vi.fn((_p: string) => Promise.resolve());
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: () => Promise.resolve(),
  openUrl: () => Promise.resolve(),
  openPath: (p: string) => openPathMock(p),
}));

import { useStore } from "../store";
import { App } from "../App";
import { ShareSheet } from "../overlays/ShareSheet";
import type { TransferDto } from "../types";

const HOUR = 3600;
const nowSecs = () => Math.floor(Date.now() / 1000);

/** A served ticket, as the daemon reports one: a send with no recipient. */
function share(over: Partial<TransferDto> = {}): TransferDto {
  return {
    id: 7,
    direction: "send",
    peer: null,
    name: "delega.pdf",
    total_size: 28234,
    transferred: 0,
    status: "active",
    swarm_peers: 0,
    pieces_from_peers: 0,
    download_peers: 0,
    created: nowSecs() - HOUR,
    code: null,
    sharing: true,
    copies_served: 0,
    bytes_served: 0,
    last_pickup: 0,
    from_download: 0,
    ...over,
  };
}

const s = () => useStore.getState();

beforeEach(() => {
  resetHarness();
  useStore.setState({
    connected: true,
    route: "transfers",
    transfers: {},
    shareOpen: null,
  } as never);
});
afterEach(cleanup);

describe("a row that is being shared, not transferred", () => {
  it("says it is shared rather than showing progress that never moves", async () => {
    harness.snapshot.transfers = [share({ transferred: 28234 })];
    render(<App />);
    expect(await screen.findByText("delega.pdf")).toBeDefined();
    expect(screen.getByText("Shared")).toBeDefined();
    // The two readings that made the old row a bug report: a full bar looks
    // finished-but-stuck, an empty one looks like a send that never started.
    expect(screen.queryByText("100%")).toBeNull();
    expect(screen.queryByText("0%")).toBeNull();
  });

  it("is a transfer again while somebody is actually pulling it", async () => {
    // The distinction is about *now*, not about the row: bytes really are moving,
    // so the progress and the rate mean what they say and should be shown.
    harness.snapshot.transfers = [share({ download_peers: 1, transferred: 14000 })];
    render(<App />);
    expect(await screen.findByText("delega.pdf")).toBeDefined();
    expect(screen.queryByText("Shared")).toBeNull();
    expect(screen.getByText("Under way")).toBeDefined();
  });
});

describe("the share panel", () => {
  it("answers the question the row cannot: has anyone taken it?", async () => {
    useStore.setState({
      transfers: {
        t7: {
          key: "t7",
          id: 7,
          dir: "out",
          name: "delega.pdf",
          size: 28234,
          transferred: 28234,
          status: "sharing",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 2,
          files: 1,
          firstSeen: Date.now(),
          rank: 1,
          copiesServed: 3,
          bytesServed: 84702,
          lastPickup: nowSecs() - 2 * HOUR,
          fromDownload: 0,
        },
      },
      shareOpen: 7,
    } as never);
    render(<ShareSheet />);
    expect(await screen.findByText("3")).toBeDefined(); // copies taken
    expect(screen.getByText("2")).toBeDefined(); // downloading now
    expect(screen.getByText(/2 hours ago/)).toBeDefined(); // last taken
    expect(screen.getByText("copies taken")).toBeDefined();
    // Copies, not people — an anonymous ticket has no identity to count.
    expect(screen.getByText(/not people/i)).toBeDefined();
  });

  it("says 'never' rather than leaving the most likely answer blank", async () => {
    useStore.setState({
      transfers: {
        t7: {
          key: "t7",
          id: 7,
          dir: "out",
          name: "delega.pdf",
          size: 28234,
          transferred: 0,
          status: "sharing",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: Date.now(),
          rank: 1,
          copiesServed: 0,
          bytesServed: 0,
          lastPickup: 0,
          fromDownload: 0,
        },
      },
      shareOpen: 7,
    } as never);
    render(<ShareSheet />);
    expect(await screen.findByText("never")).toBeDefined();
  });

  it("a share nobody asked for explains where it came from", async () => {
    // The seeding a finished download turns into. The user did not create this
    // row, so without this line it reads as a send of a file they never sent —
    // which is exactly how it was reported.
    useStore.setState({
      transfers: {
        t7: {
          key: "t7",
          id: 7,
          dir: "out",
          name: "delega.pdf",
          size: 28234,
          transferred: 0,
          status: "sharing",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: Date.now(),
          rank: 1,
          copiesServed: 0,
          bytesServed: 0,
          lastPickup: 0,
          fromDownload: nowSecs() - 3 * HOUR,
        },
      },
      shareOpen: 7,
    } as never);
    render(<ShareSheet />);
    expect(await screen.findByText(/You downloaded this/)).toBeDefined();
    // And the way to stop it happening at all is offered, not buried.
    expect(screen.getByText("Change this")).toBeDefined();
  });

  it("a share the user asked for is not told where it came from", async () => {
    useStore.setState({
      transfers: {
        t7: {
          key: "t7",
          id: 7,
          dir: "out",
          name: "photos.zip",
          size: 10,
          transferred: 0,
          status: "sharing",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: Date.now(),
          rank: 1,
          copiesServed: 0,
          bytesServed: 0,
          lastPickup: 0,
          fromDownload: 0,
        },
      },
      shareOpen: 7,
    } as never);
    render(<ShareSheet />);
    // The name is both the panel's subtitle and its heading, hence "all".
    expect((await screen.findAllByText("photos.zip")).length).toBeGreaterThan(0);
    expect(screen.queryByText(/You downloaded this/)).toBeNull();
    expect(screen.queryByText("Change this")).toBeNull();
  });

  it("stopping it asks the daemon to cancel the share", async () => {
    useStore.setState({
      transfers: {
        t7: {
          key: "t7",
          id: 7,
          dir: "out",
          name: "delega.pdf",
          size: 10,
          transferred: 0,
          status: "sharing",
          encrypted: true,
          verified: false,
          method: "p2p",
          swarmPeers: 0,
          downloadPeers: 0,
          files: 1,
          firstSeen: Date.now(),
          rank: 1,
          copiesServed: 0,
          bytesServed: 0,
          lastPickup: 0,
          fromDownload: 0,
        },
      },
      shareOpen: 7,
    } as never);
    render(<ShareSheet />);
    fireEvent.click(await screen.findByText("Stop sharing"));
    expect(harness.recorder.cancel).toContain(7);
    expect(s().shareOpen, "the panel closes with it").toBeNull();
  });
});

describe("a finished download", () => {
  /** A completed receive, as the daemon lists it — not as an event announces it. */
  function done(over: Partial<TransferDto> = {}): TransferDto {
    return {
      ...share(),
      id: 6,
      direction: "recv",
      status: "completed",
      transferred: 28234,
      sharing: false,
      path: "/Users/ls/Arvolo/delega.pdf",
      ...over,
    };
  }

  // The bug: the path only ever arrived on the `Completed` event, so a window that
  // started afterwards — or restarted — rebuilt the row from the snapshot without
  // one, and the menu quietly dropped every way of reaching the file. "Downloaded"
  // with no way to open it is the app forgetting the last useful thing it knew.
  it("can still be opened from a row the window never watched finish", async () => {
    harness.snapshot.transfers = [done()];
    render(<App />);
    expect(await screen.findByText("delega.pdf")).toBeDefined();
    const tx = useStore.getState().transfers.t6;
    expect(tx.path, "the snapshot carries where it landed").toBe(
      "/Users/ls/Arvolo/delega.pdf"
    );
  });

  it("offers both the file and the folder", async () => {
    useStore.setState({ transfers: {}, route: "transfers" } as never);
    harness.snapshot.transfers = [done()];
    render(<App />);
    await screen.findByText("delega.pdf");
    fireEvent.click(screen.getByLabelText(/Actions for delega.pdf/i));
    expect(await screen.findByText("Open file")).toBeDefined();
    expect(screen.getByText("Open the folder")).toBeDefined();
  });

  it("a row with nowhere to point offers neither", async () => {
    useStore.setState({ transfers: {}, route: "transfers" } as never);
    harness.snapshot.transfers = [done({ path: null })];
    render(<App />);
    await screen.findByText("delega.pdf");
    fireEvent.click(screen.getByLabelText(/Actions for delega.pdf/i));
    expect(screen.queryByText("Open file")).toBeNull();
    expect(screen.queryByText("Open the folder")).toBeNull();
  });

  it("double-clicking the row opens the file", async () => {
    const opened: string[] = [];
    openPathMock.mockImplementation((p: string) => {
      opened.push(p);
      return Promise.resolve();
    });
    useStore.setState({ transfers: {}, route: "transfers" } as never);
    harness.snapshot.transfers = [done()];
    render(<App />);
    const name = await screen.findByText("delega.pdf");
    fireEvent.doubleClick(name.closest(".row") as HTMLElement);
    expect(opened).toEqual(["/Users/ls/Arvolo/delega.pdf"]);
  });
});

describe("a deposited send", () => {
  /** A send that went to the mailbox, as the daemon lists it. */
  function deposited(over: Partial<TransferDto> = {}): TransferDto {
    return {
      ...share(),
      id: 9,
      peer: "p1",
      name: "conti.xlsx",
      status: "deposited",
      transferred: 28234,
      sharing: false,
      ...over,
    };
  }

  // "Waiting to be picked up" is true for a week and says nothing about whether it
  // ever reached them. The relay can now answer that, in the same words the
  // deposits panel uses — two vocabularies for one fact would be two to maintain.
  it("says it reached their device rather than just 'waiting'", async () => {
    harness.snapshot.transfers = [deposited({ offer_status: "arrived" })];
    render(<App />);
    expect(await screen.findByText("conti.xlsx")).toBeDefined();
    expect(screen.getByText(/not taken yet/i)).toBeDefined();
    expect(screen.queryByText(/waiting for the recipient/i)).toBeNull();
  });

  it("says when they took it", async () => {
    harness.snapshot.transfers = [deposited({ offer_status: "taken" })];
    render(<App />);
    expect(await screen.findByText("conti.xlsx")).toBeDefined();
    expect(screen.getByText("Taken")).toBeDefined();
  });

  // No answer is not an answer: a relay not yet asked keeps the old wording
  // rather than implying anything about the recipient.
  it("falls back to 'waiting' when nothing is known", async () => {
    harness.snapshot.transfers = [deposited({ offer_status: null })];
    render(<App />);
    expect(await screen.findByText("conti.xlsx")).toBeDefined();
    expect(screen.getByText(/waiting for the recipient/i)).toBeDefined();
  });
});

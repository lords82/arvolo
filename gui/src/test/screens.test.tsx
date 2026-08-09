// Every screen and every overlay, mounted once.
//
// A redesign's characteristic failure is not a wrong colour, it is a screen that
// throws on a state nobody tried by hand — an empty list, a null config, a
// contact with no fingerprint. A thrown render unmounts the whole React tree and
// leaves a blank window, which in a release build says nothing and offers no
// devtools to ask.
//
// So this walks the app: six routes, both themes, every overlay, with data and
// without. It asserts something identifying on each, because a component that
// renders nothing at all passes a "did not throw" check while being just as
// broken.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: () => Promise.resolve(null),
  save: () => Promise.resolve(null),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: () => Promise.resolve(),
  openUrl: () => Promise.resolve(),
  openPath: () => Promise.resolve(),
}));

import { useStore, applyTheme, type Route } from "../store";
import { App } from "../App";
import { useToasts } from "../ui/Toasts";

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
    route: "transfers",
    loadError: null,
    actionError: null,
    contacts: [],
    contactsById: {},
    transfers: {},
    deposits: [],
    history: [],
    presence: {},
    config: null,
    sync: null,
    pairing: null,
    sheetPaths: null,
    sheetTo: null,
    incomingOfferId: null,
    personOpen: null,
    paletteOpen: false,
    receiveOpen: false,
    search: "",
  });
  useToasts.setState({ items: [] });
  applyTheme("system");
}

beforeEach(() => {
  resetHarness();
  reset();
});
afterEach(() => {
  cleanup();
  applyTheme("system");
});

/** Something identifying on each screen — not just "it did not throw". */
const ROUTES: { route: Route; expect: RegExp }[] = [
  { route: "transfers", expect: /Drag the files you want to send|Outgoing/ },
  { route: "people", expect: /Swap contacts/ },
  { route: "deposits", expect: /No live link or deposit|Public links/ },
  { route: "history", expect: /Nothing yet|Today/ },
  { route: "devices", expect: /shared identity|Loading/ },
  { route: "settings", expect: /Who you are|Loading/ },
];

describe("every screen mounts", () => {
  it.each(ROUTES)("$route renders with nothing in it", async ({ route, expect: re }) => {
    render(<App />);
    useStore.getState().go(route);
    await waitFor(() => expect(screen.getAllByText(re).length).toBeGreaterThan(0));
  });

  it.each(ROUTES)("$route renders with data in it", async ({ route }) => {
    harness.snapshot.contacts = [
      dto.contact({ name: "proj", id: "p1", verified: true }),
      // A contact with no fingerprint and no display name: the shape a book
      // written by an older build leaves behind.
      dto.contact({ name: "vuoto", id: "p2", fingerprint: "", verified: false }),
    ];
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "a.bin", status: "active" }),
      dto.transfer({ id: 2, name: "b.bin", status: "completed" }),
      dto.transfer({ id: 3, name: "c.bin", status: "failed: il relay non risponde" }),
    ];
    harness.snapshot.pending = [dto.offer({ id: "o1", from: "p1", name: "in.zip" })];
    harness.snapshot.deposits = [
      {
        id: "d1",
        kind: "link",
        name: "x.pdf",
        size: 10,
        link: "https://relay.test/dl/a#k",
        ticket: "",
        recipient: "",
        created: 1,
        expires: Math.floor(Date.now() / 1000) + 3600,
        expired: false,
        max_label: "1",
        present: true,
        downloads: 0,
        max_downloads: 1,
        offer_status: null,
      },
    ];
    harness.snapshot.history = [
      {
        id: "h1",
        direction: "send",
        peer: "p1",
        name: "vecchio.zip",
        total_size: 10,
        transferred: 10,
        status: "completed",
        created: Math.floor(Date.now() / 1000) - 3600,
      },
    ];

    render(<App />);
    useStore.getState().go(route);
    // The assertion is the absence of a crash *and* the presence of the frame:
    // the error boundary would have replaced both.
    await waitFor(() =>
      expect(document.querySelector(".rail")).not.toBeNull()
    );
    expect(document.querySelector(".view")?.textContent ?? "").not.toBe("");
  });
});

describe("every overlay mounts", () => {
  it("the send sheet, in all four modes", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "proj" })];
    render(<App />);
    useStore.getState().openSheet(["/a.txt"]);
    await screen.findByText("What you are sending");
    for (const mode of ["Code", "Link", "Ticket", "To a contact"]) {
      fireEvent.click(screen.getByText(mode));
      await waitFor(() => expect(document.querySelector(".sheet")).not.toBeNull());
    }
  });

  it("the receive sheet", async () => {
    render(<App />);
    useStore.getState().openReceive();
    expect(await screen.findByPlaceholderText("4821-crater-mango")).toBeTruthy();
  });

  it("the incoming dialog, for a sender who is a total stranger", async () => {
    // The hardest case for this screen: nothing is known about them, so the
    // fingerprint has to carry the whole decision.
    harness.snapshot.pending = [
      dto.offer({ id: "o1", from: "unknown", name: "boh.zip" }),
    ];
    render(<App />);
    await screen.findByText("boh.zip");
    useStore.getState().openIncoming("o1");
    expect(await screen.findByText(/Not in the address book/)).toBeTruthy();
  });

  it("the pairing sheet, for all four kinds", async () => {
    render(<App />);
    for (const kind of [
      "contact_host",
      "contact_join",
      "device_host",
      "device_join",
    ] as const) {
      useStore.setState({
        pairing: {
          session: "p",
          kind,
          code: "4821-crater-mango",
          phase: "waiting",
          message: "",
          needsRestart: false,
        },
      });
      await waitFor(() =>
        expect(document.querySelector(".sheet")).not.toBeNull()
      );
    }
  });

  it("the command palette, and it finds a person by name", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "giulia" })];
    render(<App />);
    await waitFor(() =>
      expect(useStore.getState().contacts).toHaveLength(1)
    );
    useStore.getState().setPaletteOpen(true);
    const input = await screen.findByLabelText("Search a command or a person…");
    fireEvent.change(input, { target: { value: "giulia" } });
    await waitFor(() =>
      expect(screen.getByText("Send to giulia")).toBeTruthy()
    );
  });
});

describe("theming", () => {
  it("the explicit choice beats the system preference, both ways", () => {
    // `data-theme` has to win in both directions, which is why theme.css scopes
    // its dark block with :not([data-theme="light"]) rather than relying on the
    // media query alone.
    applyTheme("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    applyTheme("light");
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    // "system" removes the attribute entirely — a third value would leave both
    // branches unmatched.
    applyTheme("system");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });

  it("the app renders under either theme", async () => {
    for (const t of ["dark", "light"] as const) {
      applyTheme(t);
      const { unmount } = render(<App />);
      await waitFor(() =>
        expect(document.querySelector(".rail")).not.toBeNull()
      );
      unmount();
    }
  });
});

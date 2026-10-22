// The phone's own shape, driven the way a thumb drives it.
//
// Everything else in this suite runs in a window: jsdom has no `matchMedia`, so
// `useIsPhone` answers "no" and every test so far has been looking at the
// desktop's screens. That is the right default — the window is what most of the
// tests are about — and it left the five places at the bottom of a phone, the
// camera screen and the one list that replaced the board with nothing in front
// of them at all.
//
// So this file lies about the viewport, once, and then presses what is there.
// The design of record is "Arvolo Mobile 2 — Scambia", section 11: the bar is
// Attività, **Scambia**, Persone; Scambia is the one place a send or a receive
// starts from, with two questions in a fixed order — with whom, then what —
// and Impostazioni is behind the gear every root screen carries.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { dto, harness, makeIpcMock, resetHarness, PEER_ID } from "./mocks";

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
}));

/** The camera, as the plugin would answer it. `scan` never resolves, which is
 *  what a viewfinder *is*: it is waiting for something to be held up to it. */
const scan = vi.fn(
  (_opts?: { windowed?: boolean }) => new Promise<{ content: string }>(() => {})
);
const cancel = vi.fn(() => Promise.resolve());
vi.mock("@tauri-apps/plugin-barcode-scanner", () => ({
  scan: (opts?: { windowed?: boolean }) => scan(opts),
  cancel: () => cancel(),
  checkPermissions: () => Promise.resolve("granted"),
  requestPermissions: () => Promise.resolve("granted"),
  openAppSettings: () => Promise.resolve(),
  Format: { QRCode: "QR_CODE" },
}));

import { useStore } from "../store";
import { App } from "../App";
import { en } from "../i18n/en";

const s = () => useStore.getState();

beforeEach(() => {
  resetHarness();
  scan.mockClear();
  cancel.mockClear();
  // A phone, as far as the stylesheet's own breakpoint is concerned. Nothing
  // else matches: a theme query asking about `prefers-color-scheme` must still
  // get "no" rather than being told it is a phone.
  window.matchMedia = ((q: string) => ({
    matches: q.includes("560px"),
    media: q,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as never;
  useStore.setState({
    connected: true,
    route: "transfers",
    stack: ["transfers"],
    transfers: {},
    search: "",
    sheetPicks: null,
    sheetTo: null,
    sheetMode: null,
    sheetDevice: null,
    incomingOfferId: null,
    devices: [],
    contacts: [],
  } as never);
});
afterEach(cleanup);

/** A machine of yours, in the register. */
function device(over: Record<string, unknown> = {}) {
  return {
    id: "bb00",
    name: "MacBook",
    platform: "macos",
    last_seen: Math.floor(Date.now() / 1000) - 120,
    is_this_device: false,
    ...over,
  };
}

describe("the three places at the bottom", () => {
  it("names them, and the middle one is a place rather than a verb", async () => {
    render(<App />);
    const bar = await screen.findByRole("navigation");
    for (const label of [en["title.activity"], en["exchange.title"], en["title.people"]]) {
      expect(bar.textContent).toContain(label);
    }
    // Invia is not a place any more: Scambia asks "a chi" once, for both.
    expect(bar.textContent).not.toContain(en["app.send"]);
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    expect(s().route).toBe("exchange");
    expect(screen.getByTitle(en["exchange.title"]).getAttribute("aria-current")).toBe("page");
  });

  it("Impostazioni is behind the gear, and the bar goes away under it", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByLabelText(en["title.settings"]));
    expect(s().route).toBe("settings");
    expect(s().stack).toEqual(["transfers", "settings"]);
    // Pushed screens of section 12 have the title on the page and no bar.
    expect(await screen.findByText(en["title.settings"], { selector: ".page-title" })).toBeTruthy();
    expect(screen.queryByRole("navigation")).toBeNull();
  });
});

describe("Attività", () => {
  it("puts a waiting offer at the top, with the verb that opens it", async () => {
    harness.snapshot.pending = [
      dto.offer({ id: "o1", from: PEER_ID, name: "arrivo.zip", size: 48_000_000 }),
    ];
    render(<App />);
    // Not in the address book: the card says so, because that is what decides
    // whether the sentence above it is a warning or a note.
    await screen.findByText(new RegExp(en["activity.notInBook"]));
    fireEvent.click(screen.getByText(en["activity.examine"]));
    expect(s().incomingOfferId).toBe("o1");
  });

  it("a moving row says the percentage once, and the direction in colour", async () => {
    harness.snapshot.contacts = [
      { name: "marco", id: PEER_ID, fingerprint: "a-b-c", verified: true, trusted: false, blocked: false, display_name: "", pending_name: "" },
    ];
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, direction: "recv", peer: PEER_ID, name: "arrivo.zip", status: "active", transferred: 50, total_size: 100 }),
    ];
    render(<App />);
    const name = await screen.findByText("arrivo.zip");
    const row = name.closest(".act-row") as HTMLElement;
    // The wide scale (the canvas's section 9) takes the sentence off a row that
    // is simply moving: the number is in its own column and the bar is under it,
    // so "62%" in a third place was the same fact three times.
    expect(within(row).getByText("50%")).toBeTruthy();
    expect(within(row).queryByText(en["activity.from"]("marco"))).toBeNull();
    // What carries the direction instead is colour — on the glyph and on the
    // bar. Losing *that* would leave a list with no directions at all.
    expect(row.querySelector(".act-glyph.tone-in")).toBeTruthy();
    expect(row.querySelector(".prog.in")).toBeTruthy();
  });

  it("but a row that is not moving still says why, and with whom", async () => {
    harness.snapshot.contacts = [
      { name: "marco", id: PEER_ID, fingerprint: "a-b-c", verified: true, trusted: false, blocked: false, display_name: "", pending_name: "" },
    ];
    harness.snapshot.transfers = [
      dto.transfer({
        id: 2,
        direction: "send",
        peer: PEER_ID,
        name: "fermo.zip",
        status: "waiting: riprende appena possibile",
        waiting_code: "relayRetry",
        transferred: 10,
        total_size: 100,
      }),
    ];
    render(<App />);
    const name = await screen.findByText("fermo.zip");
    const row = name.closest(".act-row") as HTMLElement;
    expect(row.textContent).toContain(en["activity.to"]("marco"));
  });

  it("holds everything from the heading over the things it holds", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "invio.txt", status: "active" }),
    ];
    render(<App />);
    await screen.findByText("invio.txt");
    fireEvent.click(screen.getByText(en["activity.pauseAll"]));
    await waitFor(() => expect(harness.recorder.pause).toContain(1));
  });

  it("holding a row down says what can be done to it, and what each one costs", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "invio.txt", status: "active", transferred: 40, total_size: 100 }),
    ];
    render(<App />);
    const name = await screen.findByText("invio.txt");
    // A hold, not a swipe: the finger goes down and stays there.
    vi.useFakeTimers();
    try {
      fireEvent.touchStart(name, { touches: [{ clientX: 100, clientY: 200 }] });
      act(() => {
        vi.advanceTimersByTime(600);
      });
    } finally {
      vi.useRealTimers();
    }
    const sheet = await screen.findByRole("dialog");
    // The verb is not enough on its own: what cancelling throws away is the
    // whole reason the sheet exists rather than a menu.
    expect(sheet.textContent).toContain(en["transfers.cancelDesc"]);
    // No "leave it alone" button: touching anywhere else is that, as it is
    // everywhere on the platform. What is left in the card is the verbs.
    expect(within(sheet).getAllByRole("button").map((b) => b.textContent)).not.toContain("");
    fireEvent.click(within(sheet).getByText(en["transfers.cancel"]));
    await waitFor(() => expect(harness.recorder.cancel).toContain(1));
  });

  it("a row that is moved rather than held is a swipe, and opens nothing", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, name: "invio.txt", status: "active" }),
    ];
    render(<App />);
    const name = await screen.findByText("invio.txt");
    vi.useFakeTimers();
    try {
      fireEvent.touchStart(name, { touches: [{ clientX: 100, clientY: 200 }] });
      fireEvent.touchMove(name, { touches: [{ clientX: 60, clientY: 200 }] });
      act(() => {
        vi.advanceTimersByTime(600);
      });
    } finally {
      vi.useRealTimers();
    }
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("the record is the other half of this screen, and asking for it fetches it", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    expect(harness.recorder.listHistory).toBe(0);
    fireEvent.click(screen.getByText(en["title.history"]));
    await waitFor(() => expect(harness.recorder.listHistory).toBeGreaterThan(0));
  });
});

describe("aspetto e lingua", () => {
  /** The screen behind the wheel, one push in from Impostazioni. */
  async function openAppearance() {
    render(<App />);
    await screen.findByRole("navigation");
    act(() => {
      useStore.setState({ route: "appearance", stack: ["settings", "appearance"] });
    });
    return screen.findByText(en["settings.theme"]);
  }

  it("the theme turns on a wheel, and the row says what it landed on", async () => {
    await openAppearance();
    fireEvent.click(screen.getByText(en["settings.theme"]));
    const wheel = await screen.findByRole("dialog");
    // Turning it shows the answer behind the sheet before anything is confirmed
    // — the design is explicit about that, and it is why the preview exists.
    fireEvent.click(within(wheel).getByText(en["settings.themeDark"]));
    await waitFor(() =>
      expect(document.documentElement.getAttribute("data-theme")).toBe("dark")
    );
    fireEvent.click(within(wheel).getByText(en["common.done"]));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(useStore.getState().theme).toBe("dark");
  });

  it("Annulla puts back what was there before the wheel moved", async () => {
    await openAppearance();
    act(() => useStore.getState().setTheme("dark"));
    fireEvent.click(screen.getByText(en["settings.theme"]));
    const wheel = await screen.findByRole("dialog");
    fireEvent.click(within(wheel).getByText(en["settings.themeLight"]));
    await waitFor(() => expect(useStore.getState().theme).toBe("light"));
    fireEvent.click(within(wheel).getByText(en["common.cancel"]));
    await waitFor(() => expect(useStore.getState().theme).toBe("dark"));
  });
});

describe("foto e video, in Galleria o in Arvolo", () => {
  it("niente finisce in Galleria finché nessuno ha risposto", async () => {
    useStore.setState({ photos: "ask" });
    render(<App />);
    await screen.findByRole("navigation");
    act(() => {
      harness.emit({ completed: { id: 9, path: "/tmp/IMG_1.jpg" } });
    });
    // La regola che viene prima di ogni schermata: il trasferimento non aspetta
    // nessuna risposta, e nessuna copia parte prima che la risposta esista.
    await waitFor(() => expect(harness.recorder.photosAdded).toEqual([]));
  });

  it("e con la risposta già data, la copia parte da sola", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 9, direction: "recv", name: "IMG_1.jpg", status: "active" }),
    ];
    render(<App />);
    await screen.findByText("IMG_1.jpg");
    act(() => {
      useStore.setState({ photos: "gallery" });
      harness.emit({ completed: { id: 9, path: "/tmp/IMG_1.jpg" } });
    });
    await waitFor(() =>
      expect(harness.recorder.photosAdded).toContain("/tmp/IMG_1.jpg")
    );
  });

  it("un documento non va in Galleria nemmeno quando la risposta è «Galleria»", async () => {
    harness.snapshot.transfers = [
      dto.transfer({ id: 9, direction: "recv", name: "relazione.pdf", status: "active" }),
    ];
    render(<App />);
    // Due volte sullo schermo (la riga e la schermata Finito) è un dettaglio del
    // fixture, non il punto: basta che l'app sia partita.
    await screen.findAllByText("relazione.pdf");
    act(() => {
      useStore.setState({ photos: "gallery" });
      harness.emit({ completed: { id: 9, path: "/tmp/relazione.pdf" } });
    });
    // "I documenti non c'entrano: la Galleria non li vuole" — e il lato nativo
    // lo rifiuterebbe comunque, ma chiederglielo sarebbe già averlo deciso male.
    await waitFor(() => expect(harness.recorder.photosAdded).toEqual([]));
  });
});

describe("Scambia", () => {
  it("is a list of answers, and the camera is behind the first one", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    // Nothing is looking yet: the list is the screen.
    expect(scan).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByText(en["exchange.inFront"]));
    expect(s().route).toBe("camera");
    // Windowed: the preview goes *behind* the webview.
    await waitFor(() => expect(scan).toHaveBeenCalled());
    expect(scan.mock.calls[0][0]).toMatchObject({ windowed: true });
    act(() => s().back());
    await waitFor(() => expect(cancel).toHaveBeenCalled());
  });

  it("a tap on one of your machines goes to «cosa» with that machine already chosen", async () => {
    harness.snapshot.devices = [device({ name: "MacBook" }), device({ id: "aa", name: "here", is_this_device: true })];
    // Una libreria con dentro qualcosa: senza, la griglia non c'è e non si può
    // scegliere niente da mandare.
    harness.photos = [
      { id: "p1", name: "IMG_0001.HEIC", isVideo: false, duration: 0, thumb: "data:," },
    ];
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    fireEvent.click(await screen.findByText("MacBook"));
    // No door to open and no fingerprint to compare: the identity is shared, so
    // the destination is decided and only "what" is left to answer.
    expect(s().route).toBe("compose");
    expect(s().composeTo).toEqual({ kind: "self", device: "MacBook" });
    // Finché la mano è vuota il foglio dice cosa fare, non come arriverebbe
    // quello che non c'è (17c); scelta una foto, lo dice.
    expect(await screen.findByText(en["compose.pickHint"])).toBeTruthy();
    fireEvent.click(await screen.findByLabelText("IMG_0001.HEIC"));
    expect(await screen.findByText(en["compose.howSelf"]("MacBook"))).toBeTruthy();
  });

  it("a face goes to «cosa» with that person, and the button says who", async () => {
    harness.snapshot.contacts = [
      { name: "marco", id: PEER_ID, fingerprint: "a-b-c", verified: true, trusted: false, blocked: false, display_name: "", pending_name: "", last_exchange: 10 },
    ];
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    fireEvent.click(await screen.findByText("marco"));
    expect(s().composeTo).toEqual({ kind: "contact", name: "marco" });
    // Nothing chosen yet: the button asks for something to send.
    expect(await screen.findByText(en["compose.pickHint"])).toBeTruthy();
  });

  it("from Condividi the files come first, and the answer goes straight to the button", async () => {
    harness.snapshot.contacts = [
      { name: "marco", id: PEER_ID, fingerprint: "a-b-c", verified: true, trusted: false, blocked: false, display_name: "", pending_name: "", last_exchange: 10 },
    ];
    render(<App />);
    await screen.findByRole("navigation");
    harness.dropFiles([{ id: "f1", name: "foto.jpg", size: 2_000_000, isDir: false }]);
    // 11e: Scambia, with what you have in hand where the title was.
    await waitFor(() => expect(s().route).toBe("exchange"));
    expect(await screen.findByText(en["send.toWhom"])).toBeTruthy();
    fireEvent.click(await screen.findByText("marco"));
    // The blue sheet says what goes; the round button sends it on a tap, and
    // then you are back where you were (15): the card over the bar says the rest.
    expect(
      await screen.findByText(
        (_, el) =>
          !!el && el.classList.contains("comp-count") && (el.textContent ?? "").startsWith(en["compose.countPhotos"](1))
      )
    ).toBeTruthy();
    fireEvent.click(await screen.findByText(en["compose.goTo"]("marco")));
    await waitFor(() => expect(harness.recorder.sendTo.map((x) => x[0])).toContain("marco"));
    expect(s().route).toBe("exchange");
  });

  it("what another app shared is pulled in at launch, and Scambia opens with it in hand", async () => {
    // Landed before the page existed — the Android cold-start case.
    harness.shared = [{ id: "s1", name: "IMG_0001.jpg", size: 1_000_000, isDir: false }];
    render(<App />);
    await screen.findByRole("navigation");
    await waitFor(() => expect(s().route).toBe("exchange"));
    expect(s().composeFiles.map((f) => f.name)).toEqual(["IMG_0001.jpg"]);
    expect(await screen.findByText(en["send.toWhom"])).toBeTruthy();
    // And one that arrives while the app is up: the nudge, then the pull.
    harness.shared = [{ id: "s2", name: "note.txt", size: 12, isDir: false }];
    act(() => harness.sharedArrived());
    await waitFor(() => expect(s().composeFiles.map((f) => f.name)).toEqual(["note.txt"]));
  });

  it("the link is the one public answer, and its button says what it creates", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    // Una scheda pari a quella della camera, in cima: le due risposte che non
    // hanno un nome, una accanto all'altra (18a).
    fireEvent.click(await screen.findByText(en["exchange.linkCard"]));
    expect(s().composeTo).toEqual({ kind: "link" });
    expect(await screen.findByText(en["send.publicWord"], { selector: ".comp-name" })).toBeTruthy();
  });

  it("reads a code typed into the search field, and Ricevi asks the daemon (13e)", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    const field = await screen.findByLabelText(en["exchange.searchPlaceholder"]);
    fireEvent.change(field, { target: { value: "4821-crater-mango" } });
    // The box says what it recognised, and offers the one verb.
    expect(await screen.findByText(en["code.title.code"])).toBeTruthy();
    fireEvent.click(screen.getByText(en["code.receive"]));
    await waitFor(() => expect(harness.recorder.recv.map((x) => x[0])).toEqual(["4821-crater-mango"]));
    expect(s().route).toBe("transfers");
  });

  it("what one of your machines left is one row with Prendi, and Prendi takes it all", async () => {
    harness.snapshot.status = { ...harness.snapshot.status!, public_id: "me-id" };
    harness.snapshot.devices = [device({ name: "MacBook" })];
    harness.snapshot.pending = [
      dto.offer({ id: "o1", from: "me-id", name: "a.pdf", size: 1000 }),
      dto.offer({ id: "o2", from: "me-id", name: "b.pdf", size: 2000 }),
    ];
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    expect(await screen.findByText(en["exchange.takeTitle"](2, "MacBook"))).toBeTruthy();
    fireEvent.click(screen.getByText(en["exchange.take"]));
    await waitFor(() => expect(harness.recorder.accept.map((x) => x[0]).sort()).toEqual(["o1", "o2"]));
  });

  it("holding a face turns the faces into boxes, and Avanti goes to «cosa» with all of them (16a)", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    harness.snapshot.contacts = [
      dto.contact({ name: "marco", id: PEER_ID, last_exchange: 10 }),
      dto.contact({ name: "sara", id: "sara-id", last_exchange: 20 }),
    ];
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByTitle(en["exchange.title"]));
    const sara = await screen.findByText("sara");
    fireEvent.pointerDown(sara);
    act(() => void vi.advanceTimersByTime(600));
    fireEvent.pointerUp(sara);
    fireEvent.click(sara);
    await waitFor(() => expect(s().picking).toEqual(["sara"]));
    fireEvent.click(screen.getByText("marco"));
    expect(s().picking).toEqual(["sara", "marco"]);
    expect(await screen.findByText(en["exchange.pickedCount"](2))).toBeTruthy();
    fireEvent.click(screen.getByText(en["exchange.next"]));
    expect(s().composeTo).toEqual({ kind: "group", names: ["sara", "marco"] });
    expect(s().picking).toBeNull();
    vi.useRealTimers();
  });

  it("the options sheet turns the mailbox rules into a deposit, and «Aspetta» into a direct-only send (13d)", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "marco", id: PEER_ID })];
    render(<App />);
    await screen.findByRole("navigation");
    harness.dropFiles([{ id: "f1", name: "foto.jpg", size: 2_000_000, isDir: false }]);
    await waitFor(() => expect(s().route).toBe("exchange"));
    fireEvent.click(await screen.findByText("marco"));
    // Le opzioni si aprono dalla pastiglia della scadenza, e il foglio si
    // allunga invece di aprirne un altro sopra (17d).
    fireEvent.click(await screen.findByLabelText(en["compose.optionsOpen"]));
    // Thirty days, and the send goes to the mailbox with that rule on it.
    fireEvent.click(await screen.findByText(en["send.ttl30d"]));
    fireEvent.click(screen.getByText(en["common.done"]));
    fireEvent.click(await screen.findByText(en["compose.goTo"]("marco")));
    await waitFor(() => expect(harness.recorder.depositTo.length).toBe(1));
    expect(harness.recorder.depositTo[0][2]).toBe(30 * 86400);
  });
});

describe("the card over the bar", () => {
  it("shows what is passing on every root, and opens the sheet with Pausa and Annulla (15b, 15c)", async () => {
    harness.snapshot.contacts = [dto.contact({ name: "sara", id: PEER_ID })];
    harness.snapshot.transfers = [dto.transfer({ id: 7, name: "IMG_1.jpg", total_size: 1000, transferred: 620 })];
    render(<App />);
    await screen.findByRole("navigation");
    expect(await screen.findByText(`${en["xfer.to"]("sara")} · 62%`)).toBeTruthy();
    // Persone, same card.
    fireEvent.click(screen.getByTitle(en["title.people"]));
    const card = await screen.findByText(`${en["xfer.to"]("sara")} · 62%`);
    fireEvent.click(card);
    expect(await screen.findByText("62%")).toBeTruthy();
    fireEvent.click(screen.getByText(en["transfers.pause"]));
    await waitFor(() => expect(harness.recorder.pause).toEqual([7]));
    fireEvent.click(screen.getByText(en["transfers.cancel"]));
    await waitFor(() => expect(harness.recorder.cancel).toEqual([7]));
  });

  it("piles several, and says who is in the mailbox (16b)", async () => {
    harness.snapshot.contacts = [
      dto.contact({ name: "sara", id: PEER_ID }),
      dto.contact({ name: "paolo", id: "paolo-id" }),
    ];
    harness.snapshot.transfers = [
      dto.transfer({ id: 1, peer: PEER_ID, name: "a.jpg", total_size: 1000, transferred: 500 }),
      dto.transfer({ id: 2, peer: "paolo-id", name: "a.jpg", total_size: 1000, transferred: 0, status: "waiting: offered — waiting for them to accept", waiting_code: "offered" }),
    ];
    render(<App />);
    await screen.findByRole("navigation");
    expect(await screen.findByText(`${en["xfer.nSends"](2)} · 25%`)).toBeTruthy();
    expect(screen.getByText(`sara · ${en["xfer.inMailbox"]("paolo")}`)).toBeTruthy();
  });
});

describe("Depositi", () => {
  it("is the middle half of Attività, and choosing it asks the relay", async () => {
    harness.snapshot.deposits = [
      dto.deposit({ id: "d1", name: "catalogo-2026.zip", size: 1000, expires: Math.floor(Date.now() / 1000) + 6 * 86400 }),
    ];
    render(<App />);
    await screen.findByRole("navigation");
    fireEvent.click(screen.getByText(en["activity.left"]));
    expect(await screen.findByText("catalogo-2026.zip")).toBeTruthy();
    // A tap on a row is the way to its link, its revocation, its details.
    fireEvent.click(screen.getByText("catalogo-2026.zip"));
    const sheet = await screen.findByRole("dialog");
    expect(sheet.textContent).toContain(en["deposits.revoke"]);
  });
});

describe("Rete", () => {
  /** Una push in dentro da Impostazioni, come ci si arriva davvero. */
  const openNetwork = async () => {
    render(<App />);
    await screen.findByRole("navigation");
    act(() => {
      useStore.setState({ route: "network", stack: ["settings", "network"] });
    });
    return screen.findByText(en["settings.relayLabel"]);
  };

  it("riporta al predefinito togliendo la chiave, non riscrivendola", async () => {
    await openNetwork();
    fireEvent.click(screen.getByText(en["settings.relayReset"]));
    await waitFor(() =>
      expect(harness.recorder.setConfig).toEqual([{ relay: "clear" }])
    );
  });

  it("non offre il ritorno quando non c'è niente da togliere", async () => {
    // Il relay di serie: non è stato salvato niente, quindi rimetterlo a posto
    // non farebbe nulla — e un pulsante che non fa nulla non si disegna.
    harness.snapshot.config = {
      ...harness.snapshot.config,
      relay_configured: "",
      relay_source: "builtin",
    };
    await openNetwork();
    expect(screen.queryByText(en["settings.relayReset"])).toBeNull();
  });

  it("e nemmeno quando a decidere è l'ambiente", async () => {
    harness.snapshot.config = {
      ...harness.snapshot.config,
      relay_source: "env",
    };
    await openNetwork();
    expect(screen.queryByText(en["settings.relayReset"])).toBeNull();
  });
});

describe("il tuo indirizzo", () => {
  it("è una schermata, perché in una riga ci stava solo l'inizio", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    act(() => {
      useStore.setState({ route: "advanced", stack: ["settings", "advanced"] });
    });
    fireEvent.click(await screen.findByText(en["identity.title"]));
    expect(s().route).toBe("identity");
    // Intero: id *e* relay, senza ellissi in mezzo.
    expect(
      await screen.findByText(harness.snapshot.status!.shareable_id!)
    ).toBeTruthy();
  });

  it("e Avanzate non ripete né l'impronta né dove vive il file", async () => {
    render(<App />);
    await screen.findByRole("navigation");
    act(() => {
      useStore.setState({ route: "advanced", stack: ["settings", "advanced"] });
    });
    await screen.findByText(en["identity.title"]);
    // L'impronta è in Impostazioni, intera; il percorso del file non si apre da
    // qui e la versione non è una regolazione avanzata.
    expect(screen.queryByText(en["settings.fingerprintLabel"])).toBeNull();
    expect(screen.queryByText(en["settings.configFileLabel"])).toBeNull();
    expect(screen.queryByText(en["adv.version"])).toBeNull();
  });
});

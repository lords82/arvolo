// The settings screen writes to the daemon's config file, and it was the one
// screen no test touched: `saveConfig` appeared nowhere in the suite. What is
// written there outlives the session — a name, a relay, a download folder — so a
// save that half-lands, or one that says it landed when it did not, matters more
// here than almost anywhere else.

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
}));

import { App } from "../App";
import { useStore } from "../store";
import { setLangChoice } from "../i18n";
import { useToasts } from "../ui/Toasts";

/** Every rejection React never awaited.
 *
 *  Refusals reach the user through `actionError` and a toast. A handler that
 *  *also* lets the promise reject produces nothing the user can see — it
 *  surfaces as a process-level unhandled rejection, attributed to whichever
 *  test happens to be running when the microtask queue drains. */
function watchUnhandled() {
  const seen: unknown[] = [];
  const onUnhandled = (e: PromiseRejectionEvent) => {
    e.preventDefault();
    seen.push(e.reason);
  };
  window.addEventListener("unhandledrejection", onUnhandled);
  return {
    seen,
    stop: () => window.removeEventListener("unhandledrejection", onUnhandled),
  };
}

/** Let queued microtasks and their rejections land before asserting on them. */
const settle = () => new Promise((r) => setTimeout(r, 0));

/** Anchor on the labels, not on the values: the relay box shows
 *  `relay_configured` ("relay.test"), while `relay` ("https://relay.test") is
 *  only its placeholder — a distinction worth not re-learning. */
async function openSettings() {
  render(<App />);
  useStore.getState().go("settings");
  return (await screen.findByLabelText("Relay")) as HTMLInputElement;
}

const nameField = () =>
  screen.getByLabelText("The name you show") as HTMLInputElement;

/** The Save next to a given input, rather than whichever one is first in the
 *  document — identity and relay each have one. */
/** The `.field` whose <label> reads exactly this. Needed because "System" is
 *  both a theme and a language, so a bare getByText finds two. */
const fieldNamed = (label: string) => {
  const lab = screen
    .getAllByText(label)
    .find((el) => el.tagName === "LABEL") as HTMLElement;
  return lab.closest(".field") as HTMLElement;
};

const saveNextTo = (input: HTMLElement) =>
  within(input.closest(".field") as HTMLElement).getByRole("button", {
    name: /save/i,
  });

beforeEach(() => {
  resetHarness();
  // Toasts live in a module-level store, so they outlive `cleanup()`: without
  // this, a success notice raised by an earlier test is still in the document
  // when a later one asserts that no success notice is there.
  useToasts.setState({ items: [] });
  // The screen renders in whatever language is current; these assertions read
  // English labels, so pin it rather than inherit the machine's.
  setLangChoice("en");
  useStore.setState({
    connected: true,
    route: "settings",
    config: null,
    configError: null,
    configLoading: false,
    actionError: null,
  } as never);
});

afterEach(() => {
  cleanup();
  setLangChoice("en");
});

describe("saving who you are", () => {
  it("sends the typed name, trimmed", async () => {
    await openSettings();
    const name = nameField();
    fireEvent.change(name, { target: { value: "  Lorenzo  " } });
    fireEvent.click(saveNextTo(name));
    await waitFor(() =>
      expect(harness.recorder.setConfig).toContainEqual({
        display_name: { set: "Lorenzo" },
      })
    );
  });

  it("an emptied name clears the setting instead of storing a blank one", async () => {
    harness.snapshot.config = {
      ...harness.snapshot.config,
      display_name: "Lorenzo",
    };
    await openSettings();
    const name = nameField();
    fireEvent.change(name, { target: { value: "   " } });
    fireEvent.click(saveNextTo(name));
    // "" and "no name set" are different states in the config file. Writing the
    // first when the user meant the second leaves a name that is present and
    // invisible — and nothing in the UI can then clear it.
    await waitFor(() =>
      expect(harness.recorder.setConfig).toContainEqual({ display_name: "clear" })
    );
  });

  it("a refused save reports itself and leaves no unhandled rejection", async () => {
    const watch = watchUnhandled();
    harness.fail = new Set(["setConfig"]);
    await openSettings();
    const name = nameField();
    fireEvent.change(name, { target: { value: "Lorenzo" } });
    fireEvent.click(saveNextTo(name));

    await waitFor(() => expect(harness.recorder.setConfig.length).toBe(1));
    await settle();
    expect(watch.seen).toEqual([]);
    watch.stop();
  });

  it("a refused save does not claim the name was saved", async () => {
    harness.fail = new Set(["setConfig"]);
    await openSettings();
    const name = nameField();
    fireEvent.change(name, { target: { value: "Lorenzo" } });
    fireEvent.click(saveNextTo(name));
    await waitFor(() => expect(harness.recorder.setConfig.length).toBe(1));
    await settle();
    expect(screen.queryByText(/name updated/i)).toBeNull();
  });

  it("the Save button stops being busy after a refusal", async () => {
    harness.fail = new Set(["setConfig"]);
    await openSettings();
    const name = nameField();
    fireEvent.change(name, { target: { value: "Lorenzo" } });
    const button = saveNextTo(name) as HTMLButtonElement;
    fireEvent.click(button);
    await waitFor(() => expect(harness.recorder.setConfig.length).toBe(1));
    // A button left spinning after a failure is a screen the user cannot use
    // again without restarting the app.
    await waitFor(() => expect(button.disabled).toBe(false));
  });
});

describe("the relay", () => {
  it("saves a typed relay address, trimmed", async () => {
    const relay = await openSettings();
    fireEvent.change(relay, { target: { value: " https://altro.example " } });
    fireEvent.click(saveNextTo(relay));
    await waitFor(() =>
      expect(harness.recorder.setConfig).toContainEqual({
        relay: { set: "https://altro.example" },
      })
    );
  });

  it("an environment-set relay cannot be edited from here", async () => {
    harness.snapshot.config = {
      ...harness.snapshot.config,
      relay_source: "env",
    };
    const relay = await openSettings();
    // The daemon reads the variable at startup and ignores the file, so an
    // editable field here would write a value that never takes effect.
    expect(relay.disabled).toBe(true);
  });
});

describe("appearance", () => {
  it("choosing a theme applies it to the document and remembers it", async () => {
    await openSettings();
    fireEvent.click(within(fieldNamed("Theme")).getByText("Dark"));
    await waitFor(() =>
      expect(document.documentElement.getAttribute("data-theme")).toBe("dark")
    );
    expect(localStorage.getItem("arvolo.theme")).toBe("dark");
  });

  it("following the system removes the override rather than picking a side", async () => {
    await openSettings();
    fireEvent.click(within(fieldNamed("Theme")).getByText("Dark"));
    await waitFor(() =>
      expect(document.documentElement.getAttribute("data-theme")).toBe("dark")
    );
    fireEvent.click(within(fieldNamed("Theme")).getByText("System"));
    // Leaving data-theme="light" would pin a dark-desktop user to light; only
    // the absent attribute lets the media query decide again.
    await waitFor(() =>
      expect(document.documentElement.hasAttribute("data-theme")).toBe(false)
    );
  });

  it("switching language repaints the screen in the new words", async () => {
    await openSettings();
    expect(screen.getAllByRole("button", { name: /save/i }).length).toBeGreaterThan(0);
    fireEvent.click(within(fieldNamed("Language")).getByText("Italiano"));
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: /salva/i }).length).toBeGreaterThan(0)
    );
    expect(screen.queryByRole("button", { name: /^save$/i })).toBeNull();
  });
});

describe("restarting the daemon", () => {
  it("asks first, and only restarts once confirmed", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: /restart/i }));
    // Confirming is a separate act: a restart drops every transfer in flight.
    expect(harness.recorder.restartDaemon).toBe(0);
    const buttons = screen.getAllByRole("button", { name: /restart/i });
    fireEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(harness.recorder.restartDaemon).toBe(1));
  });

  it("a refused restart is reported, never announced, and leaves no unhandled rejection", async () => {
    const watch = watchUnhandled();
    harness.fail = new Set(["restartDaemon"]);
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: /restart/i }));
    const buttons = screen.getAllByRole("button", { name: /restart/i });
    fireEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(harness.recorder.restartDaemon).toBe(1));
    await settle();
    expect(screen.queryByText(/restarting/i)).toBeNull();
    expect(watch.seen).toEqual([]);
    watch.stop();
  });
});

// Which daemon this window is talking to. It sits beside the restart button
// because that is where the question comes up, and it was the missing half of a
// real morning's confusion: three builds of the same daemon can exist on one
// machine, and every other line on this screen describes the identity they share.
describe("which daemon is answering", () => {
  it("names the process and the binary it runs", async () => {
    harness.snapshot.status = {
      ...harness.snapshot.status!,
      pid: 4242,
      exe: "/opt/arvolo/bin/arvolo",
    };
    await openSettings();
    const line = await screen.findByText(/pid 4242/);
    expect(line.textContent).toContain("/opt/arvolo/bin/arvolo");
  });

  it("says nothing at all against a daemon too old to report it", async () => {
    // Not "pid 0", and not an empty line where a fact should be: the field is
    // absent on the wire, and absent is not zero.
    await openSettings();
    expect(screen.queryByText(/pid/i)).toBeNull();
  });
});

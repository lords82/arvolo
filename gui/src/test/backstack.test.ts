// The register that decides what the system's back takes away.
//
// Worth its own tests because both of its failures are invisible until somebody
// is holding the phone: one leaves a dialog on screen while the screen behind
// it changes, the other eats the *next* back press instead of this one — and
// from the first screen, an eaten back press means the app closes.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { hold, resetBackstack, tookBack } from "../backstack";

beforeEach(() => {
  resetBackstack();
  vi.restoreAllMocks();
});

describe("what a back press takes", () => {
  it("takes the topmost thing that is open, then the one under it", () => {
    const closes: string[] = [];
    hold(() => closes.push("sheet"));
    hold(() => closes.push("alert"));

    expect(tookBack()).toBe(true);
    expect(closes, "the alert was on top of the sheet").toEqual(["alert"]);
    expect(tookBack()).toBe(true);
    expect(closes).toEqual(["alert", "sheet"]);
  });

  it("says no when nothing is open, so the press reaches the stack", () => {
    // This is the case that has to keep working: with nothing on top, back is
    // navigation, and swallowing it would strand the user on a screen.
    expect(tookBack()).toBe(false);
  });

  it("leaves an entry to pop, or the gesture would leave the app instead", () => {
    const push = vi.spyOn(window.history, "pushState");
    hold(() => {});
    expect(push).toHaveBeenCalledTimes(1);
  });
});

describe("closing by hand", () => {
  it("takes its own entry back, and ignores the pop that causes", () => {
    // Cancel, or a menu item: the layer goes without a back press, so the entry
    // it left has to go too — and the popstate *that* fires is not somebody
    // pressing back. Counting it as one would take away whatever is underneath.
    const back = vi.spyOn(window.history, "back").mockImplementation(() => {});
    const release = hold(() => {
      throw new Error("closed by hand should never be closed again");
    });

    release();
    expect(back, "the entry it left is taken back").toHaveBeenCalledTimes(1);
    expect(tookBack(), "and the pop it causes is ours, not a back press").toBe(
      true
    );
    // The one after it belongs to the app again.
    expect(tookBack()).toBe(false);
  });

  it("does not take an entry twice when back closed it first", () => {
    const back = vi.spyOn(window.history, "back").mockImplementation(() => {});
    let closed = false;
    const release = hold(() => {
      closed = true;
    });

    expect(tookBack()).toBe(true);
    expect(closed).toBe(true);
    // React still runs the cleanup: the layer is closed, so this must do
    // nothing at all. A second `history.back()` here would pop the screen
    // underneath — the bug this whole file exists to prevent.
    release();
    expect(back).not.toHaveBeenCalled();
    expect(tookBack()).toBe(false);
  });

  it("releasing the lower of two layers leaves the upper one holding", () => {
    const closes: string[] = [];
    vi.spyOn(window.history, "back").mockImplementation(() => {});
    const releaseSheet = hold(() => closes.push("sheet"));
    hold(() => closes.push("menu"));

    releaseSheet();
    // The sheet's own pop, swallowed; then the menu, which is still open.
    expect(tookBack()).toBe(true);
    expect(closes).toEqual([]);
    expect(tookBack()).toBe(true);
    expect(closes).toEqual(["menu"]);
  });
});

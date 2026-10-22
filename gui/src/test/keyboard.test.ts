// The keyboard, as the stylesheet sees it.

import { afterEach, describe, expect, it } from "vitest";
import { installKeyboardInset } from "../keyboard";

/** A `visualViewport` the test can move. */
function fakeViewport(height: number, offsetTop = 0) {
  const listeners: Record<string, (() => void)[]> = {};
  const vv = {
    height,
    offsetTop,
    addEventListener: (k: string, f: () => void) => (listeners[k] ??= []).push(f),
    removeEventListener: (k: string, f: () => void) => {
      listeners[k] = (listeners[k] ?? []).filter((x) => x !== f);
    },
    fire: () => listeners.resize?.forEach((f) => f()),
  };
  Object.defineProperty(window, "visualViewport", { value: vv, configurable: true });
  return vv;
}

describe("keyboard inset", () => {
  let stop = () => {};
  afterEach(() => {
    stop();
    Object.defineProperty(window, "visualViewport", { value: undefined, configurable: true });
  });

  it("is zero while the viewport is whole", () => {
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    fakeViewport(800);
    stop = installKeyboardInset();
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--kb")).toBe("0px");
    expect(root.dataset.kb).toBe("0");
  });

  it("is the part of the window the keyboard took, scroll included", () => {
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    const vv = fakeViewport(800);
    stop = installKeyboardInset();
    vv.height = 480;
    vv.offsetTop = 20;
    vv.fire();
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--kb")).toBe("300px");
    expect(root.dataset.kb).toBe("1");
  });

  it("leaves nothing behind when uninstalled", () => {
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    fakeViewport(500);
    stop = installKeyboardInset();
    stop();
    stop = () => {};
    expect(document.documentElement.style.getPropertyValue("--kb")).toBe("");
    expect(document.documentElement.dataset.kb).toBeUndefined();
  });
});

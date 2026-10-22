// Where the keyboard is, as a number the stylesheet can use.
//
// A web view is not told about the keyboard the way a native screen is. On this
// platform the layout viewport keeps its height while the *visual* one shrinks
// to what is left above the keys — so a field at the bottom of a sheet, and the
// sheet's own buttons, are simply covered. `visualViewport` is where the truth
// is: the difference between the window's height and the visible one, offset
// by however far the page has been scrolled to keep the caret in view, is the
// keyboard. It is written to `--kb` on the root and the bottom-anchored things
// (the sheets, the search dock, a footer of buttons) add it to their inset.
//
// `data-kb` is the same fact as a yes/no, for the bar: a tab bar under a
// keyboard is a bar nobody can reach, and it gets out of the way.
//
// Where the viewport is resized instead (`interactive-widget=resizes-content`,
// which `index.html` asks for and some engines honour), both numbers shrink
// together and `--kb` stays 0, which is also right: fixed things have already
// moved up with the viewport.

const SETTLE_MS = 320;

export function installKeyboardInset(): () => void {
  if (typeof window === "undefined" || !window.visualViewport) return () => {};
  const vv = window.visualViewport;
  const root = document.documentElement;
  // Pinned by hand (`data-kb-pin`): the preview, showing a keyboard it does
  // not have.
  if (root.dataset.kbPin) return () => {};

  const update = () => {
    const kb = Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop));
    root.style.setProperty("--kb", `${kb}px`);
    root.dataset.kb = kb > 40 ? "1" : "0";
  };
  // And the field itself into view, once the keyboard has finished arriving:
  // asked for immediately, the browser scrolls to where the field *was* under
  // a keyboard that is still sliding in.
  const onFocus = (e: FocusEvent) => {
    const el = e.target;
    if (!(el instanceof HTMLElement)) return;
    if (!/^(INPUT|TEXTAREA)$/.test(el.tagName) && !el.isContentEditable) return;
    window.setTimeout(() => {
      if (document.activeElement === el) {
        el.scrollIntoView({ block: "center", behavior: "smooth" });
      }
    }, SETTLE_MS);
  };

  vv.addEventListener("resize", update);
  vv.addEventListener("scroll", update);
  document.addEventListener("focusin", onFocus);
  update();
  return () => {
    vv.removeEventListener("resize", update);
    vv.removeEventListener("scroll", update);
    document.removeEventListener("focusin", onFocus);
    root.style.removeProperty("--kb");
    delete root.dataset.kb;
  };
}

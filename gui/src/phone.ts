// Is this a phone-shaped screen?
//
// One question, asked once, in one place — and asked of the *viewport* rather
// than of the platform. A window dragged narrow on a desktop gets the same
// answer, which is the honest one: the layout below the breakpoint exists
// because there is no room for a rail, not because the code is running on
// Android.
//
// The breakpoint is the one the CSS uses; they are kept together deliberately,
// because a JS answer that disagreed with the stylesheet would draw a bottom bar
// beside a rail, or neither.

import { useEffect, useState } from "react";

/** Wider than any phone in portrait, narrower than any window worth a rail. */
export const PHONE_QUERY = "(max-width: 560px)";

/** Is this a phone, asked once rather than watched. For the places that need
 *  the answer outside a component — where a hook cannot go. */
export function isPhoneNow(): boolean {
  return query()?.matches ?? false;
}

function query(): MediaQueryList | null {
  // Absent under some test renderers, and "no" is the safe answer there: the
  // desktop layout is the one every existing test was written against.
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(PHONE_QUERY)
    : null;
}

export function useIsPhone(): boolean {
  const [is, setIs] = useState(() => query()?.matches ?? false);
  useEffect(() => {
    const mq = query();
    if (!mq) return;
    const on = () => setIs(mq.matches);
    on();
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return is;
}

/** Which phone this is, when the answer changes what a sentence may claim.
 *
 *  Used for one thing only: saying where a received file *ends up*. The engine
 *  stages downloads inside the app's private storage and publishes them out of
 *  it when they are whole — into the system Downloads on Android (MediaStore),
 *  into the app's Documents folder, visible in Files, on iOS. Printing the
 *  staging path instead, which is what the screen used to do, tells somebody to
 *  go and look somewhere they cannot reach and where the file will not be. */
export function phoneOs(): "android" | "ios" | "other" {
  if (typeof navigator === "undefined") return "other";
  const ua = navigator.userAgent;
  if (/Android/i.test(ua)) return "android";
  if (/iPhone|iPad|iPod/i.test(ua)) return "ios";
  return "other";
}

/** The system's text size, as a factor on the phone's own type scale.
 *
 *  Every other app on the phone follows the size chosen in Settings; a page in
 *  a web view does not, unless it asks. WebKit answers the question through
 *  the `-apple-system-body` font keyword, which resolves to the body text style
 *  at whatever size the person picked — 17 at the default. The ratio to 17 is
 *  written to `--dt` on the root, and every `font-size` in the phone layout is
 *  `calc(Npx * var(--dt))`. Text scales; the heights of controls do not — rows
 *  are `min-height` and grow with their text, buttons stay 52.
 *
 *  Clamped to [0.85, 1.35] on purpose. The five accessibility sizes go up to
 *  about 3×, and at that size a 60pt row holding 34pt text is not a layout any
 *  more; those sizes are served by the system's own zoom instead. The largest
 *  of the seven ordinary sizes is 1.35, and that one is honoured in full.
 *
 *  Measured again when the app comes back to the front, which is when a visit
 *  to Settings would have changed the answer. Where the keyword means nothing
 *  (Android, a desktop) the probe reads 16 or so and the factor stays 1: a
 *  phone that cannot be asked keeps the design's own size. */
export function installDynamicType(): () => void {
  if (typeof document === "undefined" || phoneOs() !== "ios") return () => {};
  const root = document.documentElement;
  const probe = document.createElement("span");
  probe.setAttribute("aria-hidden", "true");
  probe.style.cssText =
    "position:absolute;left:-9999px;top:0;visibility:hidden;font:-apple-system-body";
  probe.textContent = "A";
  const measure = () => {
    // Pinned by hand (`data-dt` on the root): the preview, which cannot ask
    // the browser for a size and has to be looked at under several.
    const pinned = parseFloat(root.dataset.dt ?? "");
    let px = 17 * pinned;
    if (!(px > 0)) {
      document.body.appendChild(probe);
      px = parseFloat(getComputedStyle(probe).fontSize);
      probe.remove();
    }
    const raw = px > 0 ? px / 17 : 1;
    const dt = Math.min(1.35, Math.max(0.85, raw));
    // Not when it is the default: an inline `--dt: 1` would also override a
    // preview's own setting, and the stylesheet already says 1.
    if (Math.abs(dt - 1) < 0.01) root.style.removeProperty("--dt");
    else root.style.setProperty("--dt", dt.toFixed(3));
  };
  measure();
  const again = () => {
    if (!document.hidden) measure();
  };
  document.addEventListener("visibilitychange", again);
  window.addEventListener("focus", again);
  return () => {
    document.removeEventListener("visibilitychange", again);
    window.removeEventListener("focus", again);
    root.style.removeProperty("--dt");
  };
}

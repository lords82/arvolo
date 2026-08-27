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

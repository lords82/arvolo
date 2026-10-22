// The knock a phone gives when something lands.
//
// This platform answers "it worked" with something you feel rather than
// something you have to read, and that is half of why an app of its own feels
// like one. It is also the half that costs no pixels: a confirmation you feel
// does not cover the screen it is confirming, which is exactly what was wrong
// with the banners it replaces (see `toast.echo`).
//
// Five occasions and no more. A phone that buzzes at everything is a phone
// people turn the buzzing off on, and then it says nothing when it matters:
//
//   `did`      — something finished the way you wanted. Accepting a file,
//                pairing, a send handed over.
//   `refused`  — something did not, and you need to know before you look.
//   `pressed`  — a control that *moved* under the finger without moving on
//                screen: a swipe committing, a row acting. Never an ordinary
//                tap, which the button's own highlight already answers.
//   `picked`   — a choice changed: a tab, a segment, a switch, a tick of the
//                wheel. The system's own selection tick, which is the lightest
//                thing the engine does and the one every system control gives.
//   `lifted`   — a gesture passed its threshold: the hold that opens a menu,
//                the pull that will refresh when let go. Heavier than `pressed`
//                because it answers "you have gone far enough", not "it moved".
//
// Loaded on demand and failing quietly: the plugin exists on phones and nowhere
// else, and a device with the taptic engine turned off in Settings is a person
// who has said what they want. There is nothing to report either way — every
// one of these accompanies something that is already happening.

import { isPhoneNow } from "./phone";

type Kind = "did" | "refused" | "pressed" | "picked" | "lifted";

async function tap(kind: Kind): Promise<void> {
  if (!isPhoneNow()) return;
  try {
    const h = await import("@tauri-apps/plugin-haptics");
    if (kind === "pressed") {
      await h.impactFeedback("light");
    } else if (kind === "lifted") {
      await h.impactFeedback("medium");
    } else if (kind === "picked") {
      await h.selectionFeedback();
    } else {
      await h.notificationFeedback(kind === "did" ? "success" : "error");
    }
  } catch {
    // No plugin, no engine, or the person turned it off. All the same answer.
  }
}

export const haptic = {
  did: () => void tap("did"),
  refused: () => void tap("refused"),
  pressed: () => void tap("pressed"),
  picked: () => void tap("picked"),
  lifted: () => void tap("lifted"),
};

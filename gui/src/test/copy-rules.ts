// What `docs/COPY.md` says, in a form a test can check.
//
// The rule the caps encode: a user-facing string carries the outcome and, when
// there is a choice to make, the action. The reason why goes to a second level —
// a hint line on screen, `-v` in the CLI — where somebody who wants it can find
// it and everybody else is not made to re-read it. So the caps are not a style
// preference about brevity: they are where the first level ends.
//
// Categories come from the key name because the key names already encode the
// slot. `*Hint`, `*Desc`, `*Body` are the second level and get room; `*.submit*`
// and `*.menu*` are things you click and get almost none. Where a name does not
// say, `CATEGORY` names the key outright.

export type Cat =
  | "label"
  | "button"
  | "title"
  | "status"
  | "error"
  | "secondary"
  | "confirmBody";

/** Caps in characters, measured against English. */
export const CAP: Record<Cat, number> = {
  label: 24,
  button: 28,
  title: 40,
  status: 48,
  error: 60,
  secondary: 120,
  confirmBody: 140,
};

/** The cap is written for English; the others are not English and must not be
 *  squeezed into its word lengths. German compounds are the binding case — a
 *  test that ignored this would either truncate German or never pass. */
export const LANG_SLACK: Record<string, number> = {
  en: 1.0,
  it: 1.15,
  fr: 1.2,
  de: 1.35,
};

/** Keys whose category the name does not give away.
 *
 *  The fallback below is `title`, which is the strict end — so anything landing
 *  here is a string that reads as an explanation but is not named like one. Add
 *  to this map rather than loosening the fallback: a wrong default that is
 *  generous hides real cases, a wrong default that is strict shows up at once. */
const CATEGORY: Record<string, Cat> = {
  // Body text under a heading, or a whole explanatory block on its own.
  // The build line at the foot of *You*: a whole sentence, read once, by
  // somebody who went looking for it.
  "you.footer": "secondary",
  // Le frasi di spiegazione delle sezioni 11 e 12: la riga sotto «Pubblico»
  // e l'introduzione di «Lasciati».
  "compose.publicSub": "secondary",
  // Sezioni 13–16: la riga sotto un codice riconosciuto, le note in fondo ai
  // fogli, la spiegazione dell'interruttore «Aspetta», e la riga «come arriva»
  // nel foglio blu — tutte frasi lette una volta.
  "code.sub.code": "secondary",
  "code.sub.chunk": "secondary",
  "code.sub.mailbox": "secondary",
  "code.note": "secondary",
  "compose.howWait": "secondary",
  "compose.howMailbox": "secondary",
  "compose.howGroup": "secondary",
  "compose.howSelf": "secondary",
  "compose.howLink": "secondary",
  "compose.nothingPicked": "secondary",
  // Le due righe sotto l'indirizzo in `IdentityView`: cos'è, e perché non c'è
  // un quadrato da inquadrare. Si leggono una volta, da chi è andato a
  // cercarle.
  // Il riassunto della giornata: una riga di stato, non un titolo — e da
  // quando nomina ogni uscita (non riusciti, annullati, in casella) il
  // fallback a `title` la tagliava per quello che non è.
  "activity.endedToday": "status",
  // La riga sotto i due bottoni di «Link pronto» (18c): dove ritrovare il link
  // per revocarlo. Secondo livello per costruzione — si legge una volta, da chi
  // si sta chiedendo «e se cambio idea?».
  "compose.linkWhereBack": "secondary",
  "identity.what": "secondary",
  "identity.noQr": "secondary",
  "exchange.searchPlaceholder": "secondary",
  "exchange.takeTitle": "secondary",
  "xfer.bgNote": "secondary",
  "xfer.rowNote": "secondary",
  "xfer.waitingMailbox": "secondary",
  "xfer.inMailboxWaiting": "secondary",
  "waiting.forThem": "secondary",
  // What a failed row says under its name: a sentence, read once.
  "failed.sourceMissing": "secondary",
  "failed.recipientCancelled": "secondary",
  "failed.expiredOnRelay": "secondary",
  "failed.depositGone": "secondary",
  "failed.noSpace": "secondary",
  "failed.network": "secondary",
  "activity.leftIntro": "secondary",
  "send.pickerEmpty": "secondary",
  "send.noRelay": "secondary",
  "send.linkTooMany": "secondary",
  "settings.relayLocked": "secondary",
  // The sentence under a group in the phone's settings, which is where this
  // platform puts the explanation that a desktop puts beside the field.
  "adv.copyHint": "secondary",
  "settings.downloadDirEnv": "secondary",
  // A toast's second line. `common.*` is otherwise all micro-labels, which is
  // why the prefix alone reads it as one.
  "common.afterRestart": "secondary",
  // Captions under a code, and the notice above the restart button.
  "pair.captionDevice": "secondary",
  "pair.captionContact": "secondary",
  // The same slot on the phone's code screen: what to do with the square and
  // the words under it, read once by somebody holding the phone up.
  "codes.readItOut": "secondary",
  // And the line under the address book's one filled action: what the swap is
  // for, which is the half the verb above it cannot carry.
  "people.swapBoth": "secondary",
  // The heading over the one choice in *Ricevuti* is a sentence in the canvas,
  // not a micro-label: it says who the rule covers, which is the half "Foto e
  // video" leaves out.
  "received.photosLabel": "secondary",
  // Where what just arrived went: a state, reported.
  "received.wentToGallery": "status",
  "pair.needsRestart": "secondary",
  "pair.needsReopen": "secondary",
  // The line under the code that says the exchange works from either side.
  "pair.orScanTheirs": "secondary",
  // What to do about a camera the system will not hand over.
  "scan.noPermissionHow": "secondary",
  // Where a blocked person went, said in the toast that says they were blocked.
  "people.blockedWhere": "secondary",
  // A consent checkbox: its label is the sentence being agreed to, so it is one.
  "pair.understood": "secondary",
  // States, not headings.
  "pair.waitingOther": "status",
  "devices.neverSynced": "status",
  // A tooltip, and the sentence on a consent checkbox.
  "people.claimedTip": "secondary",
  "person.compared": "secondary",
  // An intro paragraph, a banner, and a line under a stat.
  "deposits.intro": "secondary",
  "app.versionMismatch": "secondary",
  "share.fromDownload": "secondary",
  // The two second lines under the update banner. Same slot as
  // `app.versionMismatch` above: a caveat under a headline, not a headline.
  "app.updateHeldBack": "secondary",
  "app.updateDevices": "secondary",
  // What the offer dialog knows about a sender it has never been told about:
  // each one is a sentence explaining a state, sitting under the fingerprint.
  "incoming.firstTime": "secondary",
  "incoming.seenBefore": "secondary",
  "incoming.saveAsSuggested": "secondary",
  // These two land in an error slot but are not one-line errors: a refusal that
  // only said "already taken" would read as a tidiness rule, when the reason it
  // exists is the marks it protects. Both carry a floor for that reason, and a
  // floor of 70 cannot live under the `error` cap of 60.
  "incoming.nameTaken": "secondary",
  "people.addNameTaken": "secondary",
};

/** The other end of the cap, for the handful of strings where being brief is the
 *  failure. Each of these is the last thing a user reads before doing something
 *  they cannot undo — handing over their identity, downloading from a key nobody
 *  checked. "Careful." satisfies every cap in this file and tells them nothing,
 *  so these carry a minimum as well as a maximum, and what the minimum has to
 *  contain is written next to each one.
 *
 *  Floors are measured in English only: a translation that came out shorter is a
 *  translation question, not a licence to weaken the original. */
export const FLOOR: Record<string, number> = {
  // Must say that whoever types the code *becomes* you — not "gets access".
  "pair.deviceWarnRest": 90,
  "pair.joinWarnRest": 90,
  // Must say the fingerprint was never compared, and what that lets through.
  "people.confirmForceBody": 90,
  // Must say nobody has checked these words yet, and where to check them —
  // otherwise a fingerprint being *present* reads as a check already made.
  "incoming.hintStranger": 70,
  // Must say why there are no words, not just that this is an id.
  "incoming.hintRawId": 70,
  // Must say there is nothing to compare against, not merely "new".
  "incoming.firstTime": 70,
  // Must name the marks that would be dropped, and whose they are.
  "incoming.nameTaken": 70,
  "people.confirmRekeyBody": 90,
  // Must say the key *is* the identity, not merely that it is private.
  "settings.identityKeyHint": 70,
};

/** Entries nobody reads on screen, so no cap applies. The `*Kw` ones are extra
 *  words the command palette matches against — deliberately a pile of synonyms,
 *  and longer is better there. `locale.tag` is a BCP 47 tag for `Intl`. */
export function isExempt(key: string): boolean {
  return (
    /Kw$/.test(key) ||
    key === "locale.tag" ||
    key === "locale.name" ||
    key === "people.exportFilename"
  );
}

/** A number and a string no translation contains by accident, so what a function
 *  entry splices in can be subtracted back out of its length. Same trick as
 *  `i18n.test.ts`, which needs both because the build erases the parameter types
 *  and a number-shaped slot turns a string into NaN. */
const MARK_N = 424242;
const MARK_S = "«ZQX»";

/** The entry as it reaches the screen, with any interpolated value removed —
 *  a caller's file name is not part of what the copy costs. */
export function render(entry: unknown): string {
  if (typeof entry === "string") return entry;
  if (typeof entry !== "function") return "";
  const fn = entry as (...a: unknown[]) => string;
  for (const mark of [MARK_S, MARK_N] as const) {
    const args = Array.from({ length: fn.length }, (_, i) =>
      typeof mark === "number" ? mark + i : `${mark}${i}`
    );
    let out: string;
    try {
      out = fn(...args);
    } catch {
      continue;
    }
    for (const a of args) out = out.split(String(a)).join("");
    return out;
  }
  return "";
}

export function categorise(key: string): Cat {
  if (CATEGORY[key]) return CATEGORY[key];
  // Anything on a confirmation except its heading and its two buttons. Not
  // just `*Body`: `deposits.confirmRevokeLink` is a confirmation body too, and
  // naming each one here would be a list that grows with the next dialog.
  if (/confirm/i.test(key) && !/(Title|Label|Cancel)$/.test(key))
    return "confirmBody";
  // `*Foot` with them: the line under a group *is* the second level, by
  // construction — it is the slot this platform puts the explanation in.
  if (/(Hint|Desc|Body|Note|Detail|Blurb|Foot|Warn[A-Z]|explain)/i.test(key))
    return "secondary";
  if (/^(blurb|sub)/i.test(key.split(".")[1] ?? "")) return "secondary";
  if (/^store\.err/.test(key)) return "error";
  if (/^(meta|status|state|eta|until|ago|section|deposit|waiting)\./.test(key))
    return "status";
  if (/(Title|title)$/.test(key) || /\.title/.test(key)) return "title";
  if (/^(common|trust|title)\./.test(key) || /Label$/.test(key)) return "label";
  if (/(submit|menu|action)/i.test(key) || /^(palette|rail)\./.test(key))
    return "button";
  return "title";
}

// A waiting row, said in the reader's language.
//
// The reason a send is waiting is decided in Rust and shown in a React window, so
// nothing the compiler can see connects the two. It used to be prose on the wire,
// which meant "offered — waiting for them to accept" sat in English inside an
// otherwise translated interface: a sentence arriving down a socket cannot be
// looked up. The daemon now sends a code beside the sentence and this side does
// the looking up — which only works for as long as the two lists agree, and
// nothing but this file makes them.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { waitingText } from "../format";
import { en } from "../i18n/en";
import { it as itDict } from "../i18n/it";
import { fr } from "../i18n/fr";
import { de } from "../i18n/de";
import { setLangChoice } from "../i18n";
import type { UITransfer } from "../types";

/** The codes this build knows how to translate. */
const KNOWN = [
  "offered",
  "tooLarge",
  "relayRetry",
  "parked",
  "relayCapped",
  "offloadFailed",
  "noRoute",
] as const;

function row(over: Partial<UITransfer> = {}): UITransfer {
  return {
    key: "t1",
    id: 1,
    dir: "out",
    name: "file.txt",
    size: 1000,
    transferred: 0,
    status: "stalled",
    peer: "",
    verified: false,
    swarmPeers: 0,
    downloadPeers: 0,
    firstSeen: 0,
    rank: 0,
    copiesServed: 0,
    bytesServed: 0,
    lastPickup: 0,
    fromDownload: 0,
    ...over,
  } as UITransfer;
}

describe("the codes the daemon sends and the ones this window knows", () => {
  // The guard that actually matters. A new `WaitingReason` variant in Rust is a
  // new code on the wire, and a window that has never heard of it falls back to
  // English — silently, and only for the people who added the variant last.
  it("are the same list", () => {
    const rust = readFileSync(
      join(__dirname, "../../../core/src/manager/mod.rs"),
      "utf8"
    );
    // The body of `fn code()`: `WaitingReason::Offered => "offered",`
    const body = rust.slice(
      rust.indexOf("pub fn code(&self)"),
      rust.indexOf("pub fn text(&self)")
    );
    expect(body, "WaitingReason::code moved — this test can no longer read it").not.toEqual("");
    const codes = [...body.matchAll(/=>\s*"([A-Za-z]+)"/g)].map((m) => m[1]).sort();
    expect(codes).toEqual([...KNOWN].sort());
  });

  it.each(KNOWN)("%s has wording in all four languages", (code) => {
    const key = `waiting.${code}`;
    for (const dict of [en, itDict, fr, de]) {
      expect(Object.keys(dict)).toContain(key);
    }
  });
});

describe("what a waiting row says", () => {
  it("translates a code it knows", () => {
    setLangChoice("it");
    expect(waitingText(row({ waitingCode: "offered" }))).toBe(
      "offerto — in attesa che accettino"
    );
    setLangChoice("de");
    expect(waitingText(row({ waitingCode: "offered" }))).toBe(
      "angeboten — wartet auf die Annahme"
    );
    setLangChoice("en");
  });

  // The relay's message is another machine's diagnostic. Translating the frame
  // around it is the most we can honestly do.
  it("quotes the relay's own words inside a translated frame", () => {
    setLangChoice("it");
    const line = waitingText(
      row({ waitingCode: "relayRetry", waitingDetail: "mailbox full" })
    );
    expect(line).toContain("mailbox full");
    expect(line).toContain("si riprova");
    setLangChoice("en");
  });

  it("still says something when the relay named no reason", () => {
    setLangChoice("it");
    const line = waitingText(row({ waitingCode: "relayRetry" }));
    expect(line).toContain("il relay non ha potuto prenderlo in carico");
    setLangChoice("en");
  });

  // Both halves of the fallback, and the reason the sentence still travels: an
  // older daemon sends no code, a newer one may send a code this build predates.
  // English is the honest answer to both — better than a confident wrong reason,
  // and far better than a blank line.
  it("falls back to the daemon's sentence when there is no code", () => {
    setLangChoice("it");
    expect(waitingText(row({ reason: "offered — waiting for them to accept" }))).toBe(
      "offered — waiting for them to accept"
    );
  });

  it("falls back for a code from a newer daemon", () => {
    setLangChoice("it");
    expect(
      waitingText(row({ waitingCode: "somethingNew", reason: "a new reason" }))
    ).toBe("a new reason");
    setLangChoice("en");
  });

  it("says the generic line when there is neither", () => {
    setLangChoice("it");
    expect(waitingText(row())).toBe("riprende appena possibile");
    setLangChoice("en");
  });
});

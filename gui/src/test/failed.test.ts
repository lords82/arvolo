// A failed row, said in the reader's language.
//
// The same arrangement as `waiting.test.ts`, for the same reason: why a transfer
// failed is decided in Rust and shown in a React window, and nothing but this
// file keeps the two lists of codes the same.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { failText } from "../format";
import { en } from "../i18n/en";
import { it as itDict } from "../i18n/it";
import { fr } from "../i18n/fr";
import { de } from "../i18n/de";
import { setLangChoice } from "../i18n";

/** The codes this build knows how to translate. */
const KNOWN = [
  "sourceMissing",
  "recipientCancelled",
  "expiredOnRelay",
  "depositGone",
  "noSpace",
  "network",
] as const;

describe("the failure codes the daemon sends and the ones this window knows", () => {
  it("are the same list", () => {
    const rust = readFileSync(join(__dirname, "../../../core/src/manager/mod.rs"), "utf8");
    const start = rust.indexOf("impl FailReason {");
    expect(start, "FailReason moved — this test can no longer read it").toBeGreaterThan(0);
    const body = rust.slice(
      rust.indexOf("pub fn code(&self)", start),
      rust.indexOf("pub fn text(&self)", start)
    );
    // `Other` has no code, on purpose: there is nothing to look up.
    const codes = [...body.matchAll(/=>\s*"([A-Za-z]+)"/g)].map((m) => m[1]).sort();
    expect(codes).toEqual([...KNOWN].sort());
  });

  it.each(KNOWN)("%s has wording in all four languages", (code) => {
    const key = `failed.${code}`;
    for (const dict of [en, itDict, fr, de]) {
      expect(Object.keys(dict)).toContain(key);
    }
  });
});

describe("what a failed row says", () => {
  it("translates a code it knows", () => {
    setLangChoice("it");
    expect(failText("recipientCancelled", undefined, "raw")).toBe("annullato dall'altra parte");
    setLangChoice("en");
  });

  it("quotes the path inside a translated frame", () => {
    setLangChoice("it");
    const line = failText("sourceMissing", "/x/due.txt", "due.txt is not a file");
    expect(line).toContain("/x/due.txt");
    expect(line).toContain("non è più dove");
    setLangChoice("en");
  });

  // A failure without a code — or from a daemon older than the field — shows
  // the daemon's own words rather than a confident wrong translation.
  it("falls back to the daemon's sentence when there is nothing to look up", () => {
    expect(failText(undefined, undefined, "prepare send: boom")).toBe("prepare send: boom");
    expect(failText("somethingNew", undefined, "boom")).toBe("boom");
  });
});

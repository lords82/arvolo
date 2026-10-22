// 44 points, or it is not a button.
//
// The platform's floor for something a finger has to hit, and the one rule in
// this stylesheet that a screenshot cannot check: a control drawn at 32 looks
// perfectly good in a picture and is missed every third tap in a hand. jsdom
// lays nothing out, so this reads the stylesheet rather than the screen — which
// is where the number is decided anyway.
//
// Two ways to satisfy it, and both are the platform's own:
//
//   * be 44 tall, which is what a row, a tab and a filled button are;
//   * be drawn smaller and carry an invisible target — `::after` with a negative
//     inset — which is what an inline capsule and a filter pill are. Apple's own
//     inline controls are 32–34pt of ink inside 44pt of target.
//
// The list below is every interactive class in the phone layout. Adding a
// control means adding a line here, which is the point: the question "is this
// thing big enough" gets asked once, in writing, rather than never.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// Read from the project root rather than from `import.meta.url`: the test runs
// transformed, and the URL it carries is the dev server's, not a file's.
const css = readFileSync(resolve(process.cwd(), "src/theme.css"), "utf8");

/** The phone layout only: the window is driven with a pointer, which has no
 *  floor of its own. */
const PHONE = css.slice(css.indexOf("@media (max-width: 560px)"));

/** Every class in the phone layout that a finger is asked to land on. */
const TARGETS = [
  ".tab",
  ".act-tool",
  ".act-head-act",
  ".act-side",
  ".act-sheet-row",
  ".act-sheet-dismiss",
  ".cam-card",
  ".cam-back",
  ".xs-cell",
  ".xs-head-link",
  ".take-btn",
  ".code-btn",
  ".tile",
  ".comp-back",
  ".comp-go",
  ".xfer-card",
  ".xfer-done-btn",
  ".sel-go",
  ".xr-row",
  ".xs-act",
  ".xs-pause-all",
  ".opt-done",
  ".stepper > button",
  ".me-block",
  ".sheet-grab",
  ".dev-tile",
  ".people-swap",
  ".people-send",
  ".wheel-item",
  ".ios-row",
  ".nav-back",
  ".bar-btn",
  ".app-phone .btn",
  ".app-phone .btn-sm",
  ".app-phone .icon-btn",
  ".segmented-chips > button",
  ".th-now-act",
];

/** The phone layout as a flat list of rules, comments gone.
 *
 *  A tokeniser rather than a regular expression over the whole file: a selector
 *  can be one of several in a list, a rule can follow a comment or another
 *  declaration, and both of those defeated the obvious pattern — silently, by
 *  finding nothing, which is the failure mode a test like this must not have. */
const RULES: { selectors: string[]; body: string }[] = (() => {
  const text = PHONE.replace(/\/\*[\s\S]*?\*\//g, "");
  const out: { selectors: string[]; body: string }[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    const head = m[1].trim();
    if (!head || head.startsWith("@")) continue;
    out.push({
      selectors: head.split(",").map((x) => x.trim()).filter(Boolean),
      body: m[2],
    });
  }
  return out;
})();

/** One px value declared on `prop`, or null. */
function pxOf(body: string, prop: "height" | "min-height"): number | null {
  const m = new RegExp(`(?:^|[;\\s])${prop}:\\s*([^;]+)`).exec(body);
  if (!m) return null;
  const v = m[1].trim();
  if (v.includes("--wheel-item")) {
    const decl = /--wheel-item:\s*(\d+)px/.exec(PHONE);
    return decl ? Number(decl[1]) : null;
  }
  const px = /^(\d+(?:\.\d+)?)px$/.exec(v);
  return px ? Number(px[1]) : null;
}

/** The height the control is drawn at, taking the *largest* of what the
 *  stylesheet says: a later rule may raise it, and none of these lower it. */
function heightOf(selector: string): number | null {
  let best: number | null = null;
  for (const r of RULES) {
    if (!r.selectors.includes(selector)) continue;
    const h = pxOf(r.body, "min-height") ?? pxOf(r.body, "height");
    if (h !== null && (best === null || h > best)) best = h;
  }
  return best;
}

/** Whether the rule grows its target past what it draws. A negative inset is
 *  the whole mechanism: `inset: -5px 0` is 10px of target added to a 34px pill. */
function hasExpander(selector: string): boolean {
  return RULES.some(
    (r) =>
      r.selectors.includes(`${selector}::after`) && /inset:\s*-/.test(r.body)
  );
}

describe("a finger's floor", () => {
  it.each(TARGETS)("%s is 44 tall, or carries a 44 target", (sel) => {
    const h = heightOf(sel);
    // A selector with no height at all is either a row that takes its height
    // from its content (and then it is the content that has to be checked) or a
    // rule that has been renamed — both are worth failing on, because this test
    // is only worth what its list is worth.
    expect(h, `${sel}: no height declared in the phone layout`).not.toBeNull();
    if ((h as number) >= 44) return;
    expect(
      hasExpander(sel),
      `${sel} is drawn at ${h}px and has no ::after target around it`
    ).toBe(true);
  });
});

/** E il modo in cui quell'altezza si può perdere lo stesso.
 *
 *  I due pulsanti di Codici erano alti 23 con il foglio che ne dichiarava 56, e
 *  il test qui sopra non poteva accorgersene: l'altezza *dichiarata* era giusta.
 *  A toglierla era il contesto — flex item dentro flex item, in una colonna più
 *  corta del suo contenuto, dove lungo l'asse principale un elemento si
 *  restringe sotto la misura che gli hai dato. L'unica difesa è dire che non
 *  deve restringersi, e questo controlla che continui a esserci scritto. */
describe("un pulsante non si restringe sotto la sua altezza", () => {
  const flexNone = (selector: string) =>
    RULES.some(
      (r) =>
        r.selectors.includes(selector) &&
        /(^|[;\s])flex:\s*none/.test(r.body)
    );

  it.each([".phone-actions", ".phone-actions .btn"])(
    "%s è dichiarato `flex: none`",
    (selector) => {
      expect(flexNone(selector)).toBe(true);
    }
  );
});

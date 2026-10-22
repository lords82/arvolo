// Text scales with the system, or it says why not.
//
// The phone follows the size chosen in Settings through one factor, `--dt`
// (see `installDynamicType` in `phone.ts`), and it only follows it where every
// `font-size` in the phone layout is written against that factor. A plain
// `font-size: 13px` added to the phone block is a label that stays 13 while
// everything around it grows — invisible on the simulator, obvious on a phone
// with the text turned up. jsdom lays nothing out, so this reads the stylesheet.
//
// Three things are allowed to be fixed, and they are named: a count in a badge,
// which is sized to its circle; the keyboard-shortcut chip, which no phone
// shows; and the row of the disc's label, where a longer word at a bigger size
// would wrap the bar.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const css = readFileSync(resolve(process.cwd(), "src/theme.css"), "utf8");
const start = css.indexOf("@media (max-width: 560px)");
const PHONE = css.slice(start, css.indexOf("\n}\n", start) + 3);

const FIXED_ALLOWED = [".tab-count", ".kbd", ".badge"];

/** Every (selector, declaration) in the phone block that sets a size in px. */
function sizes(): { selector: string; decl: string }[] {
  const text = PHONE.replace(/\/\*[\s\S]*?\*\//g, "");
  const out: { selector: string; decl: string }[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    const selector = m[1].trim();
    if (!selector || selector.startsWith("@")) continue;
    for (const line of m[2].split(";")) {
      const d = line.trim();
      if (/^font(-size)?:/.test(d) && /\d(\.\d+)?px/.test(d)) out.push({ selector, decl: d });
    }
  }
  return out;
}

describe("dynamic type", () => {
  const all = sizes();

  it("reads the phone block", () => {
    expect(all.length).toBeGreaterThan(80);
  });

  it("every size in the phone layout is written against --dt", () => {
    const fixed = all.filter(
      (x) => !x.decl.includes("var(--dt)") && !FIXED_ALLOWED.some((f) => x.selector.includes(f))
    );
    expect(fixed.map((x) => `${x.selector} → ${x.decl}`)).toEqual([]);
  });

  it("the large title and the tab label grow, but less", () => {
    const title = all.find((x) => x.selector === ".large-title");
    expect(title?.decl).toContain("min(var(--dt), 1.15)");
    const tab = all.find((x) => /(^|,\s*)\.tab$/.test(x.selector));
    expect(tab?.decl).toContain("min(var(--dt), 1.1)");
  });

  it("a field never drops under 16px, which is where WebKit zooms the page", () => {
    const field = all.find((x) => x.selector.includes(".app-phone .input"));
    expect(field?.decl).toMatch(/max\(16px,/);
  });
});

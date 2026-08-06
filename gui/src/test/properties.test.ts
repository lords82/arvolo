// Properties, not examples.
//
// A hand-written case proves one input. A property proves a *rule* over the whole
// input space: fast-check throws hundreds of generated values at it — including the
// ones nobody thinks to write down (0, NaN, 2^53, "", lone surrogates) — and when
// it finds a counterexample it shrinks it to the smallest one that still fails.
//
// This is what "try every possibility" actually looks like for the small pure
// functions: exhaustive over the finite domains (every status, every method), and
// generative over the infinite ones (every number, every string).

import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  barClass,
  extOf,
  extTint,
  fmtBytes,
  fmtEta,
  fmtRate,
  isToday,
  methodMeta,
  metaLine,
  pct,
  sectionsFor,
  shortId,
  statusMeta,
} from "../format";
import { normalizeEvent } from "../events";
import type { Method, UIStatus, UITransfer } from "../types";

/** The tones `format.ts` is allowed to return. They are class-name suffixes
 *  resolved by `.tone-*` / `.tint-*` in theme.css — the assertions below check
 *  membership rather than a hex value on purpose: the whole point of the token
 *  indirection is that the actual colour differs between light and dark. */
const TONES = ["out", "in", "ok", "warn", "bad", "mut", "violet"];
const ALL_STATUSES: UIStatus[] = [
  "in arrivo",
  "in corso",
  "in attesa",
  "in stallo",
  "in annullamento",
  "deposited",
  "completato",
  "fallito",
  "annullato",
];
const ALL_METHODS: Method[] = ["p2p", "cloud", "link", "ticket"];

const anyStatus = fc.constantFrom(...ALL_STATUSES);
const anyMethod = fc.constantFrom(...ALL_METHODS);

/** A transfer with every field generated — the shape the board must survive. */
const anyTransfer = (over: Partial<UITransfer> = {}) =>
  fc
    .record({
      key: fc.string(),
      id: fc.nat(),
      dir: fc.constantFrom("out" as const, "in" as const),
      name: fc.string(),
      size: fc.nat(),
      transferred: fc.nat(),
      status: anyStatus,
      peer: fc.option(fc.string(), { nil: undefined }),
      reason: fc.option(fc.string(), { nil: undefined }),
      encrypted: fc.boolean(),
      verified: fc.boolean(),
      method: anyMethod,
      swarmPeers: fc.nat(),
      downloadPeers: fc.nat(),
      files: fc.nat(),
      firstSeen: fc.integer({ min: 0, max: 2 ** 42 }),
      rank: fc.nat(),
      rate: fc.option(fc.double({ min: 0, max: 1e12, noNaN: true }), { nil: undefined }),
    })
    .map((t) => ({ ...(t as unknown as UITransfer), ...over }));

describe("fmtBytes — over every number", () => {
  it("never renders NaN, undefined or an empty string", () => {
    fc.assert(
      fc.property(fc.nat(), (n) => {
        const out = fmtBytes(n);
        expect(out).toBeTruthy();
        expect(out).not.toMatch(/NaN|undefined|Infinity/);
      })
    );
  });

  it("always ends in a known unit", () => {
    fc.assert(
      fc.property(fc.nat(), (n) => {
        expect(fmtBytes(n)).toMatch(/^[\d.]+ (B|KB|MB|GB|TB)$/);
      })
    );
  });

  it("is monotonic: more bytes never reads as less", () => {
    // The property a size label exists for. Catches a unit boundary that flips.
    fc.assert(
      fc.property(fc.nat({ max: 2 ** 40 }), fc.nat({ max: 2 ** 40 }), (a, b) => {
        const [lo, hi] = a <= b ? [a, b] : [b, a];
        const bytes = (s: string) => {
          const [v, u] = s.split(" ");
          const mult: Record<string, number> = {
            B: 1,
            KB: 1024,
            MB: 1024 ** 2,
            GB: 1024 ** 3,
            TB: 1024 ** 4,
          };
          return parseFloat(v) * mult[u];
        };
        // Allow for rounding: the rendered value must not *invert* the order.
        expect(bytes(fmtBytes(lo))).toBeLessThanOrEqual(bytes(fmtBytes(hi)) * 1.05 + 1);
      })
    );
  });
});

describe("pct — over every pair", () => {
  it("is always an integer within 0..100, whatever the inputs", () => {
    fc.assert(
      fc.property(anyTransfer(), (t) => {
        const p = pct(t);
        expect(Number.isInteger(p)).toBe(true);
        expect(p).toBeGreaterThanOrEqual(0);
        expect(p).toBeLessThanOrEqual(100);
      })
    );
  });

  it("a zero-size transfer is 0%, never NaN", () => {
    fc.assert(
      fc.property(fc.nat(), (transferred) => {
        expect(pct({ ...({} as UITransfer), size: 0, transferred })).toBe(0);
      })
    );
  });
});

describe("statusMeta / methodMeta — exhaustive over their domains", () => {
  it("every status maps to a label and a known tone", () => {
    // Not a sample: this *is* every possible input.
    for (const s of ALL_STATUSES) {
      expect(statusMeta(s).text).toBeTruthy();
      expect(TONES).toContain(statusMeta(s).tone);
    }
  });

  it("every method maps to a complete chip", () => {
    for (const m of ALL_METHODS) {
      const meta = methodMeta(m);
      expect(meta.label && meta.glyph).toBeTruthy();
      expect(TONES).toContain(meta.tone);
    }
  });

  it("no arbitrary string can make methodMeta return undefined", () => {
    fc.assert(
      fc.property(fc.string(), (junk) => {
        expect(methodMeta(junk as Method)?.label).toBeTruthy();
      })
    );
  });

  it("no arbitrary string can make extTint return undefined", () => {
    fc.assert(
      fc.property(fc.string(), (junk) => {
        expect(TONES).toContain(extTint(junk));
      })
    );
  });
});

describe("extOf — over every name", () => {
  it("always returns 1..4 upper-case characters", () => {
    fc.assert(
      fc.property(fc.string(), (name) => {
        const e = extOf(name);
        expect(e.length).toBeGreaterThan(0);
        expect(e.length).toBeLessThanOrEqual(4);
        expect(e).toBe(e.toUpperCase());
      })
    );
  });

  it("is case-insensitive about the name it is given", () => {
    fc.assert(
      fc.property(fc.string(), (name) => {
        expect(extOf(name.toLowerCase())).toBe(extOf(name.toUpperCase()));
      })
    );
  });
});

describe("shortId — over every id", () => {
  it("is idempotent in the sense that it never grows the string", () => {
    fc.assert(
      fc.property(fc.string(), (id) => {
        expect(shortId(id).length).toBeLessThanOrEqual(Math.max(id.length, 16));
      })
    );
  });

  it("keeps the head and tail a person compares", () => {
    fc.assert(
      fc.property(fc.string({ minLength: 17 }), (id) => {
        const s = shortId(id);
        expect(id.startsWith(s.slice(0, 7))).toBe(true);
        expect(id.endsWith(s.slice(-6))).toBe(true);
      })
    );
  });

  it("is deterministic", () => {
    fc.assert(
      fc.property(fc.string(), (id) => {
        expect(shortId(id)).toBe(shortId(id));
      })
    );
  });
});

describe("barClass — every direction × status", () => {
  it("always names the bar and exactly one direction", () => {
    for (const dir of ["out", "in"] as const) {
      for (const status of ALL_STATUSES) {
        const parts = barClass({ ...({} as UITransfer), dir, status }).split(" ");
        expect(parts).toContain("prog");
        expect(parts).toContain(dir);
        // Never both directions: the stripe and the bar have to agree.
        expect(parts.includes("out") && parts.includes("in")).toBe(false);
      }
    }
  });
});

describe("metaLine — over every transfer", () => {
  it("never leaks NaN or undefined into the row", () => {
    fc.assert(
      fc.property(anyTransfer(), (t) => {
        expect(metaLine(t)).not.toMatch(/NaN|undefined|Infinity/);
      })
    );
  });

  it("a concluded transfer stays quiet, whatever else it carries", () => {
    fc.assert(
      fc.property(anyTransfer({ status: "completato" }), (t) => {
        expect(metaLine(t)).toBe("");
      })
    );
  });
});

describe("fmtEta / fmtRate — over every rate", () => {
  it("an ETA is either empty or a plain number with a unit", () => {
    fc.assert(
      fc.property(anyTransfer(), (t) => {
        const e = fmtEta(t);
        expect(e === "" || /^\d+ (s|min|h)$/.test(e)).toBe(true);
      })
    );
  });

  it("a rate always renders with a per-second unit", () => {
    fc.assert(
      fc.property(fc.nat(), (r) => {
        expect(fmtRate(r)).toMatch(/\/s$/);
      })
    );
  });
});

describe("isToday — over every instant", () => {
  it("anything more than a day away is never today", () => {
    fc.assert(
      fc.property(fc.integer({ min: 2, max: 10_000 }), (days) => {
        expect(isToday(Date.now() - days * 86_400_000)).toBe(false);
      })
    );
  });

  it("now is always today", () => {
    fc.assert(
      fc.property(fc.integer({ min: 0, max: 1000 }), (ms) => {
        expect(isToday(Date.now() - ms)).toBe(true);
      })
    );
  });
});

describe("sectionsFor — over every board", () => {
  it("never loses or duplicates a row: sections partition the column", () => {
    // The property that matters: a transfer you own must appear exactly once, in
    // exactly one section. Anything else is a row that vanished or doubled.
    fc.assert(
      fc.property(fc.array(anyTransfer(), { maxLength: 30 }), (rows) => {
        const unique = rows.map((r, i) => ({ ...r, key: `k${i}` }));
        for (const dir of ["out", "in"] as const) {
          const mine = unique.filter((r) => r.dir === dir);
          const placed = sectionsFor(unique, dir, "").flatMap((s) => s.items);
          expect(placed).toHaveLength(mine.length);
          expect(new Set(placed.map((r) => r.key)).size).toBe(placed.length);
        }
      })
    );
  });

  it("a section is never rendered empty", () => {
    fc.assert(
      fc.property(fc.array(anyTransfer(), { maxLength: 20 }), (rows) => {
        const unique = rows.map((r, i) => ({ ...r, key: `k${i}` }));
        for (const s of sectionsFor(unique, "out", "")) {
          expect(s.items.length).toBeGreaterThan(0);
        }
      })
    );
  });

  it("search only ever narrows — it cannot conjure a row", () => {
    fc.assert(
      fc.property(fc.array(anyTransfer(), { maxLength: 20 }), fc.string(), (rows, q) => {
        const unique = rows.map((r, i) => ({ ...r, key: `k${i}` }));
        const all = sectionsFor(unique, "out", "").flatMap((s) => s.items).length;
        const found = sectionsFor(unique, "out", q).flatMap((s) => s.items).length;
        expect(found).toBeLessThanOrEqual(all);
      })
    );
  });

  it("a row never appears in the column it does not belong to", () => {
    fc.assert(
      fc.property(fc.array(anyTransfer(), { maxLength: 20 }), (rows) => {
        const unique = rows.map((r, i) => ({ ...r, key: `k${i}` }));
        for (const dir of ["out", "in"] as const) {
          for (const item of sectionsFor(unique, dir, "").flatMap((s) => s.items)) {
            expect(item.dir).toBe(dir);
          }
        }
      })
    );
  });
});

describe("normalizeEvent — over arbitrary input", () => {
  it("never throws, whatever the daemon (or a bug) sends", () => {
    // The listener runs inside the webview: an exception here is a dead UI.
    fc.assert(
      fc.property(fc.anything(), (junk) => {
        expect(() => normalizeEvent(junk)).not.toThrow();
      })
    );
  });

  it("only ever returns null or a known type — never a half-parsed event", () => {
    fc.assert(
      fc.property(fc.anything(), (junk) => {
        const ev = normalizeEvent(junk);
        if (ev !== null) {
          expect(ALL_EVENT_TYPES).toContain(ev.type);
        }
      })
    );
  });

  it("an arbitrary single-key object is only accepted if its key is a real variant", () => {
    fc.assert(
      fc.property(fc.string(), fc.object(), (tag, fields) => {
        const ev = normalizeEvent({ [tag]: fields });
        if (ev) expect(ALL_EVENT_TYPES).toContain(tag);
      })
    );
  });
});

const ALL_EVENT_TYPES = [
  "offer_received",
  "started",
  "progress",
  "completed",
  "deposited",
  "waiting",
  "paused",
  "failed",
  "cancelled",
  "contacts_changed",
];

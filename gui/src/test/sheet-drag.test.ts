// Where a dragged sheet comes to rest.

import { describe, expect, it } from "vitest";
import fc from "fast-check";
import { settle } from "../ui/sheetDrag";

const stops = [0, 300];
const height = 600;

describe("settle", () => {
  it("rests on the nearest shape when let go still", () => {
    expect(settle(20, 0, stops, height, true)).toEqual({ kind: "stop", index: 0 });
    expect(settle(280, 0, stops, height, true)).toEqual({ kind: "stop", index: 1 });
    expect(settle(149, 0, stops, height, true)).toEqual({ kind: "stop", index: 0 });
    expect(settle(151, 0, stops, height, true)).toEqual({ kind: "stop", index: 1 });
  });

  it("a moving finger is projected ahead", () => {
    // 140 down, still: top. 140 down and moving down fast: the lower shape.
    expect(settle(140, 0, stops, height, true)).toEqual({ kind: "stop", index: 0 });
    expect(settle(140, 0.5, stops, height, true)).toEqual({ kind: "stop", index: 1 });
    // And the other way.
    expect(settle(160, -0.5, stops, height, true)).toEqual({ kind: "stop", index: 0 });
  });

  it("past the lowest shape by enough, or flicked from it, the sheet goes away", () => {
    // 300 showing at the lowest stop; 40% of that is 120.
    expect(settle(300 + 121, 0, stops, height, true)).toEqual({ kind: "dismiss" });
    expect(settle(300 + 60, 0, stops, height, true)).toEqual({ kind: "stop", index: 1 });
    expect(settle(300, 1.2, stops, height, true)).toEqual({ kind: "dismiss" });
    // A flick from the top only drops to the next shape.
    expect(settle(0, 1.2, stops, height, true)).toEqual({ kind: "stop", index: 1 });
  });

  it("a sheet that cannot be dismissed has a floor", () => {
    expect(settle(900, 2, stops, height, false)).toEqual({ kind: "stop", index: 1 });
  });

  it("further down never means a taller shape", () => {
    fc.assert(
      fc.property(
        fc.array(fc.integer({ min: 0, max: 800 }), { minLength: 1, maxLength: 4 }),
        fc.integer({ min: -200, max: 1200 }),
        fc.integer({ min: -200, max: 1200 }),
        fc.double({ min: -3, max: 3, noNaN: true }),
        (raw, a, b, vy) => {
          const s = [...new Set([0, ...raw])].sort((x, y) => x - y);
          const [lo, hi] = a < b ? [a, b] : [b, a];
          const rank = (r: ReturnType<typeof settle>) => (r.kind === "dismiss" ? Infinity : r.index);
          expect(rank(settle(hi, vy, s, 1000, true))).toBeGreaterThanOrEqual(
            rank(settle(lo, vy, s, 1000, true))
          );
        }
      )
    );
  });

  it("always answers with one of the shapes it was given", () => {
    fc.assert(
      fc.property(
        fc.array(fc.integer({ min: 0, max: 800 }), { minLength: 1, maxLength: 4 }),
        fc.integer({ min: -200, max: 1200 }),
        fc.double({ min: -3, max: 3, noNaN: true }),
        fc.boolean(),
        (raw, ty, vy, can) => {
          const s = [...new Set([0, ...raw])].sort((x, y) => x - y);
          const r = settle(ty, vy, s, 1000, can);
          if (r.kind === "stop") expect(r.index).toBeLessThan(s.length);
          else expect(can).toBe(true);
        }
      )
    );
  });
});

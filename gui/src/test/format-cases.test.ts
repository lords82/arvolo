// Case-by-case coverage of every pure function behind a row. These are the small
// ones — a size, a percent, a colour — and they are exactly where an off-by-one or
// a missing branch shows up as a wrong number in front of the user, silently. Each
// case is its own test, so a failure names the input that broke.

import { describe, expect, it } from "vitest";
import {
  barColor,
  extOf,
  extTint,
  fmtBytes,
  fmtEta,
  fmtRate,
  isToday,
  methodMeta,
  metaLine,
  pct,
  shortId,
  statusMeta,
} from "../format";
import { normalizeEvent } from "../events";
import type { Method, UIStatus, UITransfer } from "../types";

const DAY = 24 * 3600 * 1000;

function t(over: Partial<UITransfer> = {}): UITransfer {
  return {
    key: "t1",
    id: 1,
    dir: "out",
    name: "file.txt",
    size: 1000,
    transferred: 0,
    status: "in corso",
    encrypted: true,
    verified: false,
    method: "p2p",
    swarmPeers: 0,
    downloadPeers: 0,
    files: 1,
    firstSeen: Date.now(),
    rank: 1,
    ...over,
  };
}

describe("fmtBytes", () => {
  it.each([
    [0, "0 B"],
    [1, "1 B"],
    [999, "999 B"],
    [1023, "1023 B"],
    [1024, "1.0 KB"],
    [1536, "1.5 KB"],
    [10 * 1024, "10 KB"], // ≥10 drops the decimal: no false precision
    [1024 * 1024, "1.0 MB"],
    [24 * 1024 * 1024, "24 MB"],
    [10_107_245, "9.6 MB"], // the real PDF from the board
    [1024 ** 3, "1.0 GB"],
    [1024 ** 4, "1.0 TB"],
    [1024 ** 5, "1024 TB"], // past the table: stays readable, no "undefined"
  ])("fmtBytes(%i) = %s", (input, want) => {
    expect(fmtBytes(input)).toBe(want);
  });
});

describe("extOf", () => {
  it.each([
    ["file.txt", "TXT"],
    ["FILE.TXT", "TXT"],
    ["report.final.pdf", "PDF"], // last extension wins
    ["archive.tar.gz", "GZ"],
    ["noextension", "FILE"],
    ["", "FILE"],
    [".hidden", "HIDD"], // a dotfile is all "suffix"; the chip clips to 4
    ["photo.jpeg", "JPEG"],
    ["a.verylongextension", "VERY"], // clipped to fit the chip
    ["Relazione finale — 2026", "FILE"],
    ["ReportClienti (1).csv.bak", "BAK"],
  ])("extOf(%j) = %s", (input, want) => {
    expect(extOf(input)).toBe(want);
  });
});

describe("pct", () => {
  it.each([
    [0, 1000, 0],
    [1, 1000, 0], // rounds down, never shows 1% for a byte
    [500, 1000, 50],
    [999, 1000, 100],
    [1000, 1000, 100],
    [1500, 1000, 100], // clamped: never over 100
    [0, 0, 0], // no size: no divide-by-zero, no NaN
    [50, 0, 0],
    [333, 1000, 33],
    [10_107_245, 10_107_245, 100],
  ])("pct(%i/%i) = %i%%", (transferred, size, want) => {
    expect(pct(t({ transferred, size }))).toBe(want);
  });
});

describe("statusMeta", () => {
  const all: UIStatus[] = [
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
  it.each(all)("statusMeta(%s) has a label and a colour", (status) => {
    const m = statusMeta(status);
    expect(m.text.length, "a status with no label renders a blank row").toBeGreaterThan(0);
    expect(m.color).toMatch(/^#[0-9a-f]{6}$/i);
  });

  it("no two statuses share a label — they must be distinguishable", () => {
    const labels = all.map((s) => statusMeta(s).text);
    expect(new Set(labels).size).toBe(labels.length);
  });

  it("'deposited' does not claim the file was delivered", () => {
    expect(statusMeta("deposited").text).not.toMatch(/consegnat|completat/i);
  });
});

describe("methodMeta", () => {
  it.each<Method>(["p2p", "cloud", "link", "ticket"])(
    "methodMeta(%s) is complete",
    (m) => {
      const meta = methodMeta(m);
      expect(meta.label.length).toBeGreaterThan(0);
      expect(meta.glyph.length).toBeGreaterThan(0);
      expect(meta.color).toMatch(/^#/);
      expect(meta.bg).toMatch(/^#/);
    }
  );

  it("an unknown method falls back rather than rendering undefined", () => {
    expect(methodMeta("nonsense" as Method).label).toBe("Cloud");
  });

  it("each method reads differently — the chip is the whole signal", () => {
    const labels = (["p2p", "cloud", "link", "ticket"] as Method[]).map(
      (m) => methodMeta(m).label
    );
    expect(new Set(labels).size).toBe(4);
  });
});

describe("extTint", () => {
  it.each(["ZIP", "MOV", "MP4", "MKV", "PDF", "KEY", "WAV", "JPG", "PNG", "TAR"])(
    "extTint(%s) gives a background and a readable foreground",
    (ext) => {
      const [bg, fg] = extTint(ext);
      expect(bg).toMatch(/^#[0-9a-f]{6}$/i);
      expect(fg).toMatch(/^#[0-9a-f]{6}$/i);
      expect(bg).not.toBe(fg); // invisible text
    }
  );

  it("an unknown extension still gets a tint, not undefined", () => {
    const [bg, fg] = extTint("XYZZY");
    expect(bg).toMatch(/^#/);
    expect(fg).toMatch(/^#/);
  });
});

describe("barColor", () => {
  it.each([
    ["out", "in corso"],
    ["out", "in stallo"],
    ["in", "in corso"],
    ["in", "in stallo"],
  ] as const)("barColor(%s, %s) is a colour", (dir, status) => {
    expect(barColor(t({ dir, status }))).toMatch(/^#[0-9a-f]{6}$/i);
  });

  it("a send and a receive never look alike — direction is the point", () => {
    expect(barColor(t({ dir: "out" }))).not.toBe(barColor(t({ dir: "in" })));
  });

  it.each(["out", "in"] as const)(
    "a stalled %s bar is dimmer than a running one",
    (dir) => {
      expect(barColor(t({ dir, status: "in stallo" }))).not.toBe(
        barColor(t({ dir, status: "in corso" }))
      );
    }
  );
});

describe("fmtRate / fmtEta", () => {
  it.each([
    [1, "1 B/s"],
    [1024, "1.0 KB/s"],
    [44_040_192, "42 MB/s"],
  ])("fmtRate(%i) = %s", (rate, want) => {
    expect(fmtRate(rate)).toBe(want);
  });

  // An unknown or nonsensical rate must yield no ETA at all, rather than a
  // confident guess the transfer will not honour.
  it.each([0, undefined, -5, NaN])("fmtEta with rate %s says nothing", (rate) => {
    expect(fmtEta(t({ size: 1000, transferred: 0, rate: rate as number }))).toBe("");
  });

  it("an already-finished transfer has no time left", () => {
    expect(fmtEta(t({ size: 1000, transferred: 1000, rate: 100 }))).toMatch(/1 s/);
  });

  it.each([
    [1000, 100, /s$/], // 10s → seconds
    [600_000, 1000, /min$/], // 10min → minutes
    [36_000_000, 1000, /h$/], // 10h → hours
  ])("fmtEta(%i bytes @ %i B/s) uses a sensible unit", (size, rate, unit) => {
    expect(fmtEta(t({ size, transferred: 0, rate }))).toMatch(unit);
  });

  it("a size with no bytes left never yields a negative ETA", () => {
    expect(fmtEta(t({ size: 100, transferred: 500, rate: 10 }))).not.toContain("-");
  });
});

describe("isToday", () => {
  it.each([
    ["now", Date.now(), true],
    ["one second ago", Date.now() - 1000, true],
    ["yesterday", Date.now() - DAY, false],
    ["three days ago", Date.now() - 3 * DAY, false],
    ["a year ago", Date.now() - 365 * DAY, false],
  ])("isToday(%s) = %s", (_label, ms, want) => {
    expect(isToday(ms as number)).toBe(want);
  });

  it("start of today counts as today", () => {
    const midnight = new Date();
    midnight.setHours(0, 0, 0, 0);
    expect(isToday(midnight.getTime())).toBe(true);
  });

  it("a minute before midnight is yesterday, not 'today'", () => {
    const justBefore = new Date();
    justBefore.setHours(0, 0, 0, 0);
    expect(isToday(justBefore.getTime() - 60_000)).toBe(false);
  });
});

describe("metaLine", () => {
  it.each<[UIStatus, RegExp]>([
    ["in corso", /%/],
    ["in attesa", /pausa/i],
    ["in stallo", /riprende|relay/i],
    ["in arrivo", /dettagli/i],
    ["deposited", /ritir/i],
    ["fallito", /fallit|boom/i],
  ])("metaLine for %s says something useful", (status, want) => {
    expect(metaLine(t({ status, reason: status === "fallito" ? "boom" : undefined }))).toMatch(
      want
    );
  });

  it.each<UIStatus>(["completato", "annullato"])(
    "a concluded row (%s) adds no noise",
    (status) => {
      expect(metaLine(t({ status }))).toBe("");
    }
  );

  it("speed and ETA only appear once a rate is known", () => {
    expect(metaLine(t({ transferred: 500, rate: undefined }))).toBe("50%");
    expect(metaLine(t({ transferred: 500, rate: 100 }))).toContain("/s");
  });
});

describe("shortId", () => {
  const full = "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha";
  it.each([
    ["", ""],
    ["short", "short"],
    ["sixteenchars1234", "sixteenchars1234"], // at the limit: left alone
  ])("shortId(%j) is untouched when it already fits", (input, want) => {
    expect(shortId(input)).toBe(want);
  });

  it("a real id keeps both ends, which is what people compare", () => {
    expect(shortId(full)).toBe("if2xmne…c7daha");
  });

  it("shortening is stable — the same id always renders the same", () => {
    expect(shortId(full)).toBe(shortId(full));
  });

  it("two different ids do not collapse to the same label", () => {
    const other = "zz2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7dzz";
    expect(shortId(full)).not.toBe(shortId(other));
  });
});

describe("normalizeEvent — every variant, one case each", () => {
  it.each([
    [{ started: { id: 1, direction: "send", name: "a", total_size: 2 } }, "started"],
    [{ progress: { id: 1, transferred: 1, total_size: 2 } }, "progress"],
    [{ completed: { id: 1, path: null } }, "completed"],
    [{ deposited: { id: 1 } }, "deposited"],
    [{ waiting: { id: 1, reason: "r" } }, "waiting"],
    [{ paused: { id: 1, reason: "r" } }, "paused"],
    [{ failed: { id: 1, error: "e" } }, "failed"],
    [{ cancelled: { id: 1 } }, "cancelled"],
    [
      { offer_received: { id: "o", from: "p", name: "n", size: 1, note: "", sender_name: "" } },
      "offer_received",
    ],
    ["contacts_changed", "contacts_changed"],
  ])("%j → type %s", (wire, type) => {
    expect(normalizeEvent(wire)?.type).toBe(type);
  });

  it.each([
    [null],
    [undefined],
    [0],
    [""],
    ["not_an_event"],
    [[]],
    [{}],
    [{ a: 1, b: 2 }],
    [{ started: null }],
    [{ type: "started", id: 1 }], // the invented shape must stay rejected
  ])("junk %j is dropped, not forwarded", (junk) => {
    expect(normalizeEvent(junk)).toBeNull();
  });
});

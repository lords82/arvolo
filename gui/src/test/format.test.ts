// What the user actually reads on a row: the section it lands in, the search that
// finds it, the size/speed/ETA, and the status wording. Pure derivations, so they
// are cheap to pin down — and they carried real bugs (a status whose reason text
// crushed the layout, a "consegnato al relay" that claimed a delivery that had not
// happened).

import { describe, expect, it } from "vitest";
import {
  extOf,
  fmtBytes,
  fmtEta,
  fmtRate,
  metaLine,
  pct,
  sectionsFor,
  shortId,
  statusMeta,
} from "../format";
import type { UIStatus, UITransfer } from "../types";

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
    rank: Date.now(),
    ...over,
  };
}

describe("sections", () => {
  it("25. offers, live rows, today and earlier land in the right sections, in order", () => {
    const rows = [
      t({ key: "o1", id: 0, offerId: "o1", dir: "in", status: "in arrivo" }),
      t({ key: "t1", dir: "in", status: "in corso" }),
      t({ key: "t2", dir: "in", status: "completato" }),
      t({ key: "t3", dir: "in", status: "completato", firstSeen: Date.now() - 3 * DAY }),
    ];
    const secs = sectionsFor(rows, "in", "");
    expect(secs.map((s) => s.title)).toEqual([
      "Da confermare",
      "In corso e in attesa",
      "Oggi",
      "Precedenti",
    ]);
  });

  it("26. an empty section is dropped, not rendered blank", () => {
    const secs = sectionsFor([t({ status: "in corso" })], "out", "");
    expect(secs).toHaveLength(1);
    expect(secs[0].title).toBe("In corso e in attesa");
  });

  it("27. a row only appears in its own column", () => {
    const rows = [t({ key: "a", dir: "out" }), t({ key: "b", dir: "in" })];
    expect(sectionsFor(rows, "out", "")[0].items.map((r) => r.key)).toEqual(["a"]);
    expect(sectionsFor(rows, "in", "")[0].items.map((r) => r.key)).toEqual(["b"]);
  });

  it("28. a deposit awaiting pickup is not filed as still running", () => {
    const secs = sectionsFor([t({ status: "deposited" })], "out", "");
    expect(secs[0].title).toBe("Oggi");
  });

  it("29. search matches the file name and the peer, case-insensitively", () => {
    const rows = [
      t({ key: "a", name: "Relazione.pdf", peer: "proj" }),
      t({ key: "b", name: "foto.zip", peer: "Marta" }),
    ];
    expect(sectionsFor(rows, "out", "relaz")[0].items.map((r) => r.key)).toEqual(["a"]);
    expect(sectionsFor(rows, "out", "marta")[0].items.map((r) => r.key)).toEqual(["b"]);
    expect(sectionsFor(rows, "out", "zzz")).toHaveLength(0);
  });
});

describe("row wording", () => {
  it("30. every status has a label and a colour (no blank/undefined row)", () => {
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
    for (const s of all) {
      const m = statusMeta(s);
      expect(m.text, s).toBeTruthy();
      expect(
        ["out", "in", "ok", "warn", "bad", "mut", "violet"],
        s
      ).toContain(m.tone);
    }
  });

  it("31. a deposit says it is awaiting pickup, never that it was delivered", () => {
    const meta = metaLine(t({ status: "deposited" }));
    expect(meta).toMatch(/ritir/i);
    expect(meta).not.toMatch(/consegnat/i);
  });

  it("32. an in-flight row shows percent, and speed/ETA once known", () => {
    expect(metaLine(t({ transferred: 500, size: 1000 }))).toBe("50%");
    const withRate = metaLine(t({ transferred: 500, size: 1000, rate: 100 }));
    expect(withRate).toContain("50%");
    expect(withRate).toContain("/s");
  });

  it("33. a stalled row surfaces the daemon's reason", () => {
    expect(metaLine(t({ status: "in stallo", reason: "relay 500" }))).toBe("relay 500");
  });

  it("34. percent is clamped and never divides by zero", () => {
    expect(pct(t({ size: 0, transferred: 0 }))).toBe(0);
    expect(pct(t({ size: 100, transferred: 999 }))).toBe(100);
  });
});

describe("units", () => {
  it("35. bytes read as humans write them", () => {
    expect(fmtBytes(0)).toBe("0 B");
    expect(fmtBytes(999)).toBe("999 B");
    expect(fmtBytes(1024)).toBe("1.0 KB");
    expect(fmtBytes(24 * 1024 * 1024)).toBe("24 MB");
  });

  it("36. rate and ETA are human, and absent when unknown", () => {
    expect(fmtRate(1024)).toBe("1.0 KB/s");
    expect(fmtEta(t({ size: 1000, transferred: 0, rate: 0 }))).toBe("");
    expect(fmtEta(t({ size: 100_000, transferred: 0, rate: 1000 }))).toMatch(/min|s/);
  });

  it("37. the extension chip stays short and upper-case", () => {
    expect(extOf("report.final.pdf")).toBe("PDF");
    expect(extOf("noext")).toBe("FILE");
    expect(extOf("a.jpeg").length).toBeLessThanOrEqual(4);
  });

  it("38. a public id is shortened but still recognisable at both ends", () => {
    const id = "if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha";
    const short = shortId(id);
    expect(short).toContain("…");
    expect(short.startsWith("if2xmne")).toBe(true);
    expect(short.endsWith("c7daha")).toBe(true);
    expect(shortId("short")).toBe("short");
  });
});

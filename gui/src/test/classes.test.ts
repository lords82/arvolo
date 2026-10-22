// Ogni classe scritta nel markup ha una regola che la disegna.
//
// Questo test esiste per un difetto vero, non per igiene. *Impostazioni* e
// *Attività › Depositi* sono state consegnate con le righe disegnate dal
// browser invece che dal foglio: erano scritte contro `big-row`, `row-glyph`,
// `big-row-title`, `r60` — un vocabolario inventato leggendo il canvas e mai
// aggiunto a `theme.css`. Un `<button>` senza regole non sparisce e non dà
// errore: diventa una pillola grigia col testo centrato. Compila, passa i test,
// e si vede solo guardandola.
//
// Né TypeScript né i test di rendering possono accorgersene — per loro una
// stringa in `className` è una stringa — quindi serviva qualcosa che leggesse
// le due metà e le confrontasse. È questa.
//
// **Cosa NON controlla.** Non che una regola sia giusta, non che sia raggiunta,
// non che la classe sia usata: solo che una classe nominata nel markup esista
// da qualche parte nel foglio. È il minimo che distingue «disegnato male» da
// «non disegnato affatto», che è la differenza fra un difetto e una schermata
// rotta.

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";

const SRC = resolve(process.cwd(), "src");
const css = readFileSync(join(SRC, "theme.css"), "utf8");

/** Ogni classe che il foglio nomina, ovunque e in qualunque combinazione. */
const DEFINED = new Set(
  [...css.matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)].map((m) => m[1])
);

/** I sorgenti che disegnano: i test e l'anteprima non vestono niente. */
function sources(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) {
      if (e.name !== "test") sources(p, out);
    } else if (/\.tsx?$/.test(e.name) && !e.name.startsWith("preview-")) {
      out.push(p);
    }
  }
  return out;
}

/** Via quello che il codice compone a runtime, prima di leggere qualunque cosa.
 *
 *  `` className={`view${isPhone ? " staged" : ""}`} `` ha delle virgolette
 *  *dentro* l'interpolazione, e un lettore che si fermi alla prima virgoletta
 *  taglia la stringa nel mezzo e si porta a casa `isPhone` come se fosse una
 *  classe. Tolte le interpolazioni per intero — con le graffe contate, non con
 *  un'espressione regolare — quello che resta sono solo i pezzi letterali, che
 *  è esattamente ciò che si può controllare. */
function withoutInterpolations(text: string): string {
  let out = "";
  let i = 0;
  for (;;) {
    const j = text.indexOf("${", i);
    if (j < 0) return out + text.slice(i);
    out += text.slice(i, j);
    let depth = 1;
    let k = j + 2;
    while (k < text.length && depth > 0) {
      if (text[k] === "{") depth++;
      else if (text[k] === "}") depth--;
      k++;
    }
    i = k;
  }
}

/** Le classi scritte a mano in un `className`. */
function classesIn(text: string): string[] {
  const out: string[] = [];
  for (const m of withoutInterpolations(text).matchAll(
    /className=\{?[`"]([^`"]*)[`"]/g
  )) {
    for (const tok of m[1].split(/\s+/)) {
      const c = tok.trim();
      if (c && !/[^\w-]/.test(c)) out.push(c);
    }
  }
  return out;
}

/** Nomi che compaiono in un `className` senza essere classi di questo foglio. */
const NOT_OURS = new Set([
  // Prefissi e suffissi che il codice completa a runtime.
  "tone-",
  "dir-",
  "tint-",
  // Parole che il tokenizer non può distinguere da una classe perché stanno in
  // un ternario senza spazi intorno — sono valori, non nomi di classe.
  "className",
  "kind",
  "cls",
  "size",
  "count",
  "state",
  "n",
  "i",
]);

describe("ogni classe del markup è disegnata dal foglio", () => {
  const files = sources(SRC);

  it("legge davvero il foglio e i sorgenti", () => {
    // Una guardia sulla guardia: se un giorno il percorso cambia e questi due
    // insiemi diventano vuoti, il test passerebbe sempre senza controllare
    // niente — che è il modo in cui muoiono i test come questo.
    expect(DEFINED.size).toBeGreaterThan(400);
    expect(files.length).toBeGreaterThan(20);
    expect(DEFINED.has("ios-row")).toBe(true);
  });

  it("non lascia nessuna classe senza regola", () => {
    const missing = new Map<string, Set<string>>();
    for (const f of files) {
      for (const c of classesIn(readFileSync(f, "utf8"))) {
        if (DEFINED.has(c) || NOT_OURS.has(c) || /^\d+$/.test(c)) continue;
        if (!missing.has(c)) missing.set(c, new Set());
        missing.get(c)!.add(f.slice(SRC.length + 1));
      }
    }
    expect(
      [...missing].map(([c, where]) => `.${c} — ${[...where].join(", ")}`)
    ).toEqual([]);
  });
});

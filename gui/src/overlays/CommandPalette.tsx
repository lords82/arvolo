// ⌘K: one place that reaches everything.
//
// Two kinds of result, ranked in that order: **actions** (send, receive, pair,
// navigate) and **people** (send to them, open their card). People are in here
// because "invia a Giulia" is one intent, and making it a navigation followed by
// a click is making the user assemble it out of parts.
//
// Matching is a plain case-insensitive substring over a keyword string per
// entry. Nothing fuzzier: with two dozen commands, fuzzy matching mostly buys
// surprising results, and a palette that offers the wrong thing confidently is
// worse than one that offers nothing.

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { fire, useStore, type Route } from "../store";
import { useModal } from "../ui/Sheet";
import { Icon } from "../ui/Icons";
import { Avatar } from "../ui/Bits";

interface Entry {
  key: string;
  label: string;
  hint?: string;
  icon: JSX.Element;
  /** Extra words that should match this entry but need not be shown. */
  keywords?: string;
  group: string;
  run: () => void;
}

export function CommandPalette() {
  const open = useStore((s) => s.paletteOpen);
  const setOpen = useStore((s) => s.setPaletteOpen);
  const go = useStore((s) => s.go);
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const startPairing = useStore((s) => s.startPairing);
  const setTheme = useStore((s) => s.setTheme);
  const theme = useStore((s) => s.theme);
  const contacts = useStore((s) => s.contacts);
  const openPerson = useStore((s) => s.openPerson);
  const syncNow = useStore((s) => s.syncNow);
  const clearFinished = useStore((s) => s.clearFinished);
  const togglePauseAll = useStore((s) => s.togglePauseAll);
  const pauseAll = useStore((s) => s.pauseAll);

  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const paletteRef = useRef<HTMLDivElement>(null);

  // It calls itself aria-modal; it has to actually be one. Without this, Tab
  // walked out into the rail behind it and Escape only worked while focus was
  // still in the input.
  useModal(paletteRef, () => setOpen(false), open);

  const entries = useMemo<Entry[]>(() => {
    const nav = (route: Route, label: string, icon: JSX.Element, kw?: string): Entry => ({
      key: `go:${route}`,
      label,
      icon,
      keywords: kw,
      group: "Vai a",
      run: () => go(route),
    });

    const list: Entry[] = [
      {
        key: "send",
        label: "Invia file…",
        hint: "contatto, codice, link o ticket",
        icon: <Icon.Send />,
        keywords: "manda spedisci upload nuovo",
        group: "Azioni",
        run: () => openSheet([]),
      },
      {
        key: "receive",
        label: "Ricevi…",
        hint: "incolla un codice o un ticket",
        icon: <Icon.Receive />,
        keywords: "scarica download incolla",
        group: "Azioni",
        run: openReceive,
      },
      {
        key: "pair-contact",
        label: "Scambia contatti con qualcuno",
        hint: "vi salvate a vicenda, già verificati",
        icon: <Icon.Qr />,
        keywords: "pairing accoppia aggiungi persona verifica",
        group: "Azioni",
        run: () => fire(startPairing("contact_host")),
      },
      {
        key: "pair-device",
        label: "Collega un altro tuo dispositivo",
        icon: <Icon.Devices />,
        keywords: "multidevice identita sincronizza",
        group: "Azioni",
        run: () => fire(startPairing("device_host")),
      },
      {
        key: "sync",
        label: "Sincronizza la rubrica adesso",
        icon: <Icon.Refresh />,
        keywords: "contatti dispositivi",
        group: "Azioni",
        run: () => fire(syncNow()),
      },
      {
        key: "pause-all",
        label: pauseAll
          ? "Riprendi tutti i trasferimenti"
          : "Metti in pausa tutti i trasferimenti",
        icon: pauseAll ? <Icon.Play /> : <Icon.Pause />,
        keywords: "pausa tutto ferma sospendi riprendi",
        group: "Azioni",
        run: () => fire(togglePauseAll()),
      },
      {
        key: "clear-finished",
        label: "Pulisci i trasferimenti conclusi",
        icon: <Icon.Trash />,
        keywords: "svuota completati",
        group: "Azioni",
        run: () => fire(clearFinished()),
      },
      nav("transfers", "Trasferimenti", <Icon.Transfers />, "board invii"),
      nav("people", "Persone", <Icon.People />, "contatti rubrica"),
      nav("deposits", "Link e depositi", <Icon.Link />, "relay revoca"),
      nav("history", "Cronologia", <Icon.History />, "storico log"),
      nav("devices", "I tuoi dispositivi", <Icon.Devices />, "sync identita"),
      nav("settings", "Impostazioni", <Icon.Settings />, "config relay nome"),
      {
        key: "theme",
        label:
          theme === "dark"
            ? "Passa al tema chiaro"
            : theme === "light"
              ? "Segui il tema di sistema"
              : "Passa al tema scuro",
        icon: theme === "dark" ? <Icon.Sun /> : <Icon.Moon />,
        keywords: "tema scuro chiaro dark light aspetto",
        group: "Azioni",
        run: () =>
          setTheme(theme === "dark" ? "light" : theme === "light" ? "system" : "dark"),
      },
    ];

    for (const c of contacts) {
      if (c.blocked) continue;
      list.push({
        key: `to:${c.name}`,
        label: `Invia a ${c.name}`,
        hint: c.verified ? "verificato" : "non verificato",
        icon: <Avatar name={c.display_name || c.name} id={c.id} size={18} />,
        keywords: `${c.display_name} ${c.id}`,
        group: "Persone",
        run: () => openSheet([], c.name),
      });
      list.push({
        key: `open:${c.name}`,
        label: `Apri la scheda di ${c.name}`,
        icon: <Icon.Info />,
        keywords: `${c.display_name} impronta fingerprint verifica`,
        group: "Persone",
        run: () => {
          go("people");
          openPerson(c.name);
        },
      });
    }
    return list;
  }, [
    go,
    openSheet,
    openReceive,
    startPairing,
    setTheme,
    theme,
    contacts,
    openPerson,
    syncNow,
    clearFinished,
    togglePauseAll,
    pauseAll,
  ]);

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return entries.slice(0, 12);
    return entries
      .filter((e) =>
        `${e.label} ${e.hint ?? ""} ${e.keywords ?? ""}`
          .toLowerCase()
          .includes(needle)
      )
      .slice(0, 24);
  }, [entries, q]);

  useEffect(() => {
    if (open) {
      setQ("");
      setActive(0);
      // A frame later: the input does not exist until this render commits.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  useEffect(() => setActive(0), [q]);

  // Keep the highlighted row in view when arrowing past the fold.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>('[data-active="true"]');
    el?.scrollIntoView({ block: "nearest" });
  }, [active]);

  if (!open) return null;

  const pick = (i: number) => {
    const e = shown[i];
    if (!e) return;
    setOpen(false);
    e.run();
  };

  let lastGroup = "";

  return (
    <>
      <div className="scrim" onClick={() => setOpen(false)} />
      <div
        ref={paletteRef}
        className="palette"
        role="dialog"
        aria-modal="true"
        aria-label="Cerca ed esegui"
      >
        <input
          ref={inputRef}
          value={q}
          onChange={(e) => setQ(e.currentTarget.value)}
          placeholder="Cerca un comando o una persona…"
          aria-label="Cerca un comando o una persona"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={shown[active] ? `pal-${shown[active].key}` : undefined}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              setOpen(false);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              setActive((i) => Math.min(i + 1, shown.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((i) => Math.max(i - 1, 0));
            } else if (e.key === "Enter") {
              e.preventDefault();
              pick(active);
            }
          }}
        />
        <div className="list" id="palette-list" role="listbox" ref={listRef}>
          {shown.length === 0 && (
            <div className="t-sm t-mut" style={{ padding: 18, textAlign: "center" }}>
              Niente corrisponde a «{q}».
            </div>
          )}
          {shown.map((e, i) => {
            const header = e.group !== lastGroup ? e.group : null;
            lastGroup = e.group;
            return (
              <Fragment key={e.key}>
                {header && (
                  <div className="t-label group-label" role="presentation">
                    {header}
                  </div>
                )}
                <button
                  id={`pal-${e.key}`}
                  role="option"
                  aria-selected={i === active}
                  className="opt"
                  tabIndex={-1}
                  data-active={i === active}
                  onMouseEnter={() => setActive(i)}
                  onClick={() => pick(i)}
                >
                  <span className="ico">{e.icon}</span>
                  <span className="truncate">{e.label}</span>
                  {e.hint && <span className="sub truncate">{e.hint}</span>}
                </button>
              </Fragment>
            );
          })}
        </div>
      </div>
    </>
  );
}

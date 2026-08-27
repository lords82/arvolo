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
import { fire, TITLE_KEY, useStore, type Route } from "../store";
import { useT } from "../i18n";
import { useModal } from "../ui/Sheet";
import { Icon } from "../ui/Icons";
import { Avatar, claimText } from "../ui/Bits";

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
  const t = useT();
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
    const nav = (route: Route, icon: JSX.Element, kw: string): Entry => ({
      key: `go:${route}`,
      // The same label the rail and the header use — one name per place.
      label: t(TITLE_KEY[route]),
      icon,
      keywords: kw,
      group: t("palette.groupGoTo"),
      run: () => go(route),
    });

    const actions = t("palette.groupActions");
    const people = t("palette.groupPeople");

    const list: Entry[] = [
      {
        key: "send",
        label: t("palette.send"),
        hint: t("palette.sendHint"),
        icon: <Icon.Send />,
        keywords: t("palette.sendKw"),
        group: actions,
        run: () => openSheet([]),
      },
      {
        key: "receive",
        label: t("palette.receive"),
        hint: t("palette.receiveHint"),
        icon: <Icon.Receive />,
        keywords: t("palette.receiveKw"),
        group: actions,
        run: openReceive,
      },
      {
        key: "pair-contact",
        label: t("palette.pairContact"),
        hint: t("palette.pairContactHint"),
        icon: <Icon.Qr />,
        keywords: t("palette.pairContactKw"),
        group: actions,
        run: () => fire(startPairing("contact_host")),
      },
      {
        key: "pair-device",
        label: t("palette.pairDevice"),
        icon: <Icon.Devices />,
        keywords: t("palette.pairDeviceKw"),
        group: actions,
        run: () => fire(startPairing("device_host")),
      },
      {
        key: "sync",
        label: t("palette.sync"),
        icon: <Icon.Refresh />,
        keywords: t("palette.syncKw"),
        group: actions,
        run: () => fire(syncNow()),
      },
      {
        key: "pause-all",
        label: pauseAll ? t("palette.resumeAll") : t("palette.pauseAll"),
        icon: pauseAll ? <Icon.Play /> : <Icon.Pause />,
        keywords: t("palette.pauseAllKw"),
        group: actions,
        run: () => fire(togglePauseAll()),
      },
      {
        key: "clear-finished",
        label: t("palette.clearFinished"),
        icon: <Icon.Trash />,
        keywords: t("palette.clearFinishedKw"),
        group: actions,
        run: () => fire(clearFinished()),
      },
      nav("transfers", <Icon.Transfers />, t("palette.navTransfersKw")),
      nav("people", <Icon.People />, t("palette.navPeopleKw")),
      nav("deposits", <Icon.Link />, t("palette.navDepositsKw")),
      nav("history", <Icon.History />, t("palette.navHistoryKw")),
      nav("devices", <Icon.Devices />, t("palette.navDevicesKw")),
      nav("settings", <Icon.Settings />, t("palette.navSettingsKw")),
      {
        key: "theme",
        label:
          theme === "dark"
            ? t("palette.themeLight")
            : theme === "light"
              ? t("palette.themeSystem")
              : t("palette.themeDark"),
        icon: theme === "dark" ? <Icon.Sun /> : <Icon.Moon />,
        keywords: t("palette.themeKw"),
        group: actions,
        run: () =>
          setTheme(theme === "dark" ? "light" : theme === "light" ? "system" : "dark"),
      },
    ];

    for (const c of contacts) {
      if (c.blocked) continue;
      // `hint` is a string here, not JSX, so the claim joins the trust mark on
      // one line rather than getting its own styling. Same words as the row.
      const claim = claimText(c, t);
      const trust = c.verified ? t("palette.verified") : t("palette.notVerified");
      list.push({
        key: `to:${c.name}`,
        label: t("palette.sendTo", c.name),
        hint: claim ? `${trust} · ${claim.text}` : trust,
        icon: <Avatar name={c.display_name || c.name} id={c.id} size={18} />,
        keywords: `${c.display_name} ${c.pending_name} ${c.id}`,
        group: people,
        run: () => openSheet([], c.name, "contact"),
      });
      list.push({
        key: `open:${c.name}`,
        label: t("palette.openCard", c.name),
        hint: claim?.text,
        icon: <Icon.Info />,
        keywords: `${c.display_name} ${c.pending_name} ${t("palette.personKw")}`,
        group: people,
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
    t,
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
        aria-label={t("palette.label")}
      >
        <input
          ref={inputRef}
          value={q}
          onChange={(e) => setQ(e.currentTarget.value)}
          placeholder={t("palette.placeholder")}
          aria-label={t("palette.placeholder")}
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
              {t("palette.noMatch", q)}
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

// The frame: rail, header, one view, and every overlay that can sit on top.
//
// Three things live here because they are properties of the *window* rather than
// of any screen: the drag-and-drop target (a file dropped anywhere means "send
// this", whatever is showing), the global keyboard map, and the two banners that
// report the app's own health — a daemon that cannot be reached, and a daemon
// whose version no longer matches this build.

import { useEffect, useMemo, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { fire, useStore, type Route } from "./store";
import { Icon } from "./ui/Icons";
import { Button, IconButton, modKey, TextInput } from "./ui/Primitives";
import { ToastHost, toast } from "./ui/Toasts";
import { Rail } from "./views/Rail";
import { TransfersView } from "./views/TransfersView";
import { PeopleView } from "./views/PeopleView";
import { DepositsView } from "./views/DepositsView";
import { HistoryView } from "./views/HistoryView";
import { DevicesView } from "./views/DevicesView";
import { SettingsView } from "./views/SettingsView";
import { SendSheet } from "./overlays/SendSheet";
import { ReceiveSheet } from "./overlays/ReceiveSheet";
import { IncomingDialog } from "./overlays/IncomingDialog";
import { PairSheet } from "./overlays/PairSheet";
import { CommandPalette } from "./overlays/CommandPalette";

const TITLES: Record<Route, string> = {
  transfers: "Trasferimenti",
  people: "Persone",
  deposits: "Link e depositi",
  history: "Cronologia",
  devices: "I tuoi dispositivi",
  settings: "Impostazioni",
};

/** Only the board is searchable in place; the other screens carry their own
 *  filter controls, and a header field that silently did nothing on four of six
 *  screens would be worse than no field. */
const SEARCHABLE: Route[] = ["transfers"];

export function App() {
  const init = useStore((s) => s.init);
  const connected = useStore((s) => s.connected);
  const status = useStore((s) => s.status);
  const guiVersion = useStore((s) => s.guiVersion);
  const loadError = useStore((s) => s.loadError);
  const actionError = useStore((s) => s.actionError);
  const dismissActionError = useStore((s) => s.dismissActionError);
  const reload = useStore((s) => s.reload);
  const route = useStore((s) => s.route);
  const go = useStore((s) => s.go);
  const search = useStore((s) => s.search);
  const setSearch = useStore((s) => s.setSearch);
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const openIncoming = useStore((s) => s.openIncoming);
  const restartDaemon = useStore((s) => s.restartDaemon);
  const setPaletteOpen = useStore((s) => s.setPaletteOpen);
  const clearFinished = useStore((s) => s.clearFinished);
  const transfers = useStore((s) => s.transfers);

  const [dragging, setDragging] = useState(false);

  // Select the stable map, derive the array with useMemo — a selector returning
  // a fresh array every call makes useSyncExternalStore loop forever.
  const pendingOffers = useMemo(
    () => Object.values(transfers).filter((t) => t.status === "in arrivo"),
    [transfers]
  );
  const finishedCount = useMemo(
    () =>
      Object.values(transfers).filter(
        (t) =>
          t.status === "completato" ||
          t.status === "annullato" ||
          t.status === "fallito"
      ).length,
    [transfers]
  );

  // The CLI refuses to talk to a mismatched daemon; the GUI shows a banner
  // instead (an old daemon reports version "" here).
  const versionMismatch =
    connected && status !== null && guiVersion !== "" && status.version !== guiVersion;

  // Boot the store (snapshot + event subscription) once. `init` is async, so an
  // unmount can land before it resolves — track that and dispose the listeners
  // it hands back, or they'd leak (and double up under StrictMode's re-mount).
  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    init().then((c) => {
      if (disposed) c();
      else cleanup = c;
    });
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [init]);

  // Native feel: no webview context menu (right-click → Reload/Inspect), and in
  // production also swallow the reload / devtools keyboard shortcuts. Text inputs
  // keep normal keyboard editing (Cmd/Ctrl+C/V still work).
  useEffect(() => {
    const onCtx = (e: MouseEvent) => e.preventDefault();
    const onKey = (e: KeyboardEvent) => {
      if (import.meta.env.DEV) return; // keep hot-reload while developing
      const k = e.key.toLowerCase();
      const isReload = (e.metaKey || e.ctrlKey) && k === "r";
      const devtools =
        k === "f12" ||
        ((e.metaKey || e.ctrlKey) && e.shiftKey && (k === "i" || k === "c" || k === "j")) ||
        (e.metaKey && e.altKey && k === "i");
      if (isReload || devtools || k === "f5") e.preventDefault();
    };
    document.addEventListener("contextmenu", onCtx);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("contextmenu", onCtx);
      document.removeEventListener("keydown", onKey);
    };
  }, []);

  // App shortcuts. Deliberately skipped while focus is in a text field: ⌘N in
  // the middle of typing a note should not throw the note away.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const typing =
        el instanceof HTMLInputElement ||
        el instanceof HTMLTextAreaElement ||
        (el as HTMLElement | null)?.isContentEditable === true;
      const mod = e.metaKey || e.ctrlKey;
      const k = e.key.toLowerCase();

      if (mod && k === "k") {
        e.preventDefault();
        setPaletteOpen(true);
        return;
      }
      if (typing) return;
      if (mod && k === "n") {
        e.preventDefault();
        openSheet([]);
      } else if (mod && k === "v" && !e.shiftKey) {
        // ⌘V outside a field means "I have a code on the clipboard".
        e.preventDefault();
        openReceive();
      } else if (mod && k === ",") {
        e.preventDefault();
        go("settings");
      } else if (k === "/" ) {
        e.preventDefault();
        document.getElementById("app-search")?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [setPaletteOpen, openSheet, openReceive, go]);

  // Refuse the webview's own drag-and-drop default, everywhere in the document.
  //
  // Not a nicety: releasing a file on a page that does not cancel `drop` makes the
  // webview *navigate to it*, which replaces the entire app with a blank page and
  // no way back. It has to be the document rather than the drop zone, because the
  // gesture that breaks it is precisely the one that misses the zone.
  useEffect(() => {
    const stop = (e: DragEvent) => e.preventDefault();
    document.addEventListener("dragover", stop);
    document.addEventListener("drop", stop);
    return () => {
      document.removeEventListener("dragover", stop);
      document.removeEventListener("drop", stop);
    };
  }, []);

  // Drag and drop, window-wide. Tauri delivers real filesystem paths here, which
  // is the whole reason sending works by dropping: a browser File object would
  // have no path for the daemon to read.
  useEffect(() => {
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type === "over" || e.payload.type === "enter") {
          setDragging(true);
        } else if (e.payload.type === "drop") {
          setDragging(false);
          const paths = e.payload.paths ?? [];
          if (paths.length) openSheet(paths);
        } else {
          setDragging(false);
        }
      })
      .then((f) => {
        un = f;
      })
      .catch(() => {
        // No drag-drop in this webview build; the picker still works.
      });
    return () => un?.();
  }, [openSheet]);

  // A refused action is surfaced as a toast rather than left in the store: it is
  // fired from click handlers that cannot await, so without this the button
  // simply looks broken. Errors never auto-dismiss (see Toasts).
  useEffect(() => {
    if (!actionError) return;
    toast.bad("Non ha funzionato", actionError);
    dismissActionError();
  }, [actionError, dismissActionError]);

  const View =
    route === "transfers"
      ? TransfersView
      : route === "people"
        ? PeopleView
        : route === "deposits"
          ? DepositsView
          : route === "history"
            ? HistoryView
            : route === "devices"
              ? DevicesView
              : SettingsView;

  return (
    <div className="app">
      <Rail />

      <div className="main">
        {/* --- health banners ------------------------------------------ */}
        {!connected && (
          <div className="banner bad" role="status">
            <Icon.Alert size={14} className="tone-bad" />
            <span className="grow">
              Non riesco a parlare con il daemon. I trasferimenti in corso
              proseguono, ma questa finestra non li vede.
            </span>
            <Button size="sm" onClick={() => fire(reload())}>
              Riprova
            </Button>
          </div>
        )}
        {versionMismatch && (
          <div className="banner" role="status">
            <Icon.Alert size={14} className="tone-warn" />
            <span className="grow">
              Il daemon in esecuzione è la versione {status?.version || "precedente"},
              l'app è la {guiVersion}. Riavvialo per allinearli.
            </span>
            <Button size="sm" onClick={() => fire(restartDaemon())}>
              Riavvia
            </Button>
          </div>
        )}
        {loadError && (
          <div className="banner bad" role="status">
            <Icon.Alert size={14} className="tone-bad" />
            <span className="grow">{loadError}</span>
          </div>
        )}
        {pendingOffers.length > 0 && route !== "transfers" && (
          <div className="banner" role="status">
            <Icon.Receive size={14} className="tone-in" />
            <span className="grow">
              {pendingOffers.length === 1
                ? "Qualcuno vuole mandarti un file."
                : `${pendingOffers.length} file in attesa della tua conferma.`}
            </span>
            <Button
              size="sm"
              variant="in"
              onClick={() =>
                pendingOffers.length === 1
                  ? openIncoming(pendingOffers[0].offerId!)
                  : go("transfers")
              }
            >
              Vedi
            </Button>
          </div>
        )}

        {/* --- header --------------------------------------------------- */}
        <header className="header">
          <h1>{TITLES[route]}</h1>
          <div className="spacer" />

          {SEARCHABLE.includes(route) && (
            <div style={{ width: 240 }}>
              <TextInput
                id="app-search"
                value={search}
                onChange={(e) => setSearch(e.currentTarget.value)}
                placeholder="Filtra per nome o persona…"
                aria-label="Filtra i trasferimenti"
              />
            </div>
          )}

          {route === "transfers" && finishedCount > 0 && (
            <Button size="sm" onClick={() => fire(clearFinished())}>
              <Icon.Trash size={13} />
              Pulisci ({finishedCount})
            </Button>
          )}

          <IconButton
            label={`Cerca ed esegui (${modKey}K)`}
            onClick={() => setPaletteOpen(true)}
          >
            <Icon.Search size={15} />
          </IconButton>
          <Button
            size="sm"
            variant="primary"
            onClick={() => openSheet([])}
            title={`Invia (${modKey}N)`}
          >
            <Icon.Send size={13} />
            Invia
          </Button>
        </header>

        {/* --- the screen ----------------------------------------------- */}
        <main className="view">
          <View />
        </main>
      </div>

      {/* --- overlays ---------------------------------------------------- */}
      <SendSheet />
      <ReceiveSheet />
      <IncomingDialog />
      <PairSheet />
      <CommandPalette />
      <ToastHost />

      {dragging && (
        <div className="drop-overlay">
          <div className="inner">
            <Icon.Send size={34} className="tone-out" />
            <div className="t-title">Lascia qui per inviare</div>
            <div className="t-sm t-mut">
              Poi scegli a chi: un contatto, un codice, un link.
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

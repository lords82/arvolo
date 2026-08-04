import { useEffect, useMemo, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "./store";
import { Board } from "./components/Board";
import { Sidebar } from "./components/Sidebar";
import { SendSheet } from "./components/SendSheet";
import { IncomingModal } from "./components/IncomingModal";
import { DepositsPanel } from "./components/DepositsPanel";
import { ReceiveModal } from "./components/ReceiveModal";
import { ContactsPanel } from "./components/ContactsPanel";
import { HistoryPanel } from "./components/HistoryPanel";

export function App() {
  const init = useStore((s) => s.init);
  const connected = useStore((s) => s.connected);
  const status = useStore((s) => s.status);
  const guiVersion = useStore((s) => s.guiVersion);
  const loadError = useStore((s) => s.loadError);
  const actionError = useStore((s) => s.actionError);
  const dismissActionError = useStore((s) => s.dismissActionError);
  const reload = useStore((s) => s.reload);
  const openSheet = useStore((s) => s.openSheet);
  const openIncoming = useStore((s) => s.openIncoming);
  const restartDaemon = useStore((s) => s.restartDaemon);
  const historyOpen = useStore((s) => s.historyOpen);
  const contactsOpen = useStore((s) => s.contactsOpen);
  const depositsOpen = useStore((s) => s.depositsOpen);
  // Select the stable map, derive the array with useMemo — a selector returning
  // a fresh array every call makes useSyncExternalStore loop forever.
  const transfers = useStore((s) => s.transfers);
  const pendingOffers = useMemo(
    () => Object.values(transfers).filter((t) => t.status === "in arrivo"),
    [transfers]
  );
  const [dragging, setDragging] = useState(false);

  // Stale-daemon detection: the CLI refuses to talk to a mismatched daemon; the
  // GUI shows a banner instead (an old daemon reports version "" here).
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
      const reload = (e.metaKey || e.ctrlKey) && k === "r";
      const devtools =
        k === "f12" ||
        ((e.metaKey || e.ctrlKey) && e.shiftKey && (k === "i" || k === "c" || k === "j")) ||
        (e.metaKey && e.altKey && k === "i");
      if (reload || devtools || k === "f5") e.preventDefault();
    };
    document.addEventListener("contextmenu", onCtx);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("contextmenu", onCtx);
      document.removeEventListener("keydown", onKey);
    };
  }, []);

  // Refuse the browser's drag machinery everywhere. A file released on a spot the
  // app does not handle would otherwise hit the webview's default — navigate to the
  // file — replacing the app with a blank page and no way back. Tauri reports real
  // OS drops through its own event below, so nothing here needs the DOM's version.
  useEffect(() => {
    const swallow = (e: Event) => e.preventDefault();
    const events = ["dragenter", "dragover", "dragleave", "drop"] as const;
    events.forEach((ev) => window.addEventListener(ev, swallow));
    return () => events.forEach((ev) => window.removeEventListener(ev, swallow));
  }, []);

  // Real OS file drops (from Finder/Explorer/Nautilus), not just HTML drag.
  useEffect(() => {
    const p = getCurrentWebview().onDragDropEvent((event) => {
      const pl = event.payload;
      if (pl.type === "over" || pl.type === "enter") setDragging(true);
      else if (pl.type === "leave") setDragging(false);
      else if (pl.type === "drop") {
        setDragging(false);
        if (pl.paths && pl.paths.length) openSheet(pl.paths);
      }
    });
    return () => {
      p.then((un) => un());
    };
  }, [openSheet]);

  async function pick() {
    const sel = await open({ multiple: true, directory: false });
    if (!sel) return;
    openSheet(Array.isArray(sel) ? sel : [sel]);
  }

  const viewTitle = depositsOpen
    ? "Depositi"
    : historyOpen
      ? "Storico"
      : contactsOpen
        ? "Rubrica"
        : "Trasferimenti";
  const boardView = !depositsOpen && !historyOpen && !contactsOpen;

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        background: "var(--canvas)",
        minWidth: 0,
      }}
    >
      {/* The whole window takes a drop, not just the dashed strip. Tauri reports OS
          drops for the entire webview, so a file released anywhere already reaches
          us; saying so plainly is the difference between "it did nothing" and an
          obvious target. (Releasing it outside a handled area used to make the
          webview navigate to the file and blank the window — see main.tsx.) */}
      {dragging && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            zIndex: 200,
            background: "rgba(249,115,22,.10)",
            border: "3px dashed var(--out)",
            borderRadius: 12,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: 10,
            pointerEvents: "none",
            backdropFilter: "blur(1px)",
          }}
        >
          <div
            style={{
              width: 56,
              height: 56,
              borderRadius: 16,
              background: "var(--out)",
              color: "#fff",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 26,
            }}
          >
            ↑
          </div>
          <div style={{ fontSize: 15, fontWeight: 700 }}>
            Rilascia per scegliere il destinatario
          </div>
          <div style={{ fontSize: 12.5, color: "#8a6d3b" }}>
            Puoi rilasciare ovunque nella finestra
          </div>
        </div>
      )}

      <Sidebar />

      {/* main column: slim topbar, banners, then the active view */}
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          minWidth: 0,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "12px 18px 8px",
            flex: "none",
          }}
        >
          <span style={{ fontSize: 16, fontWeight: 700 }}>{viewTitle}</span>
          <div style={{ marginLeft: "auto" }}>
            {/* Arrivals bell: red badge = offers awaiting a decision. Lives up
                here — not in the sidebar — so it is reachable from every view. */}
            <button
              onClick={() =>
                pendingOffers.length && openIncoming(pendingOffers[0].offerId!)
              }
              title={
                pendingOffers.length
                  ? `${pendingOffers.length} file in arrivo da confermare`
                  : "Nessun arrivo in attesa"
              }
              style={{
                position: "relative",
                border: "1px solid var(--line-strong)",
                background: "var(--card)",
                borderRadius: 8,
                width: 30,
                height: 30,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: pendingOffers.length ? "pointer" : "default",
                color: pendingOffers.length ? "var(--ink)" : "var(--ink-mut)",
              }}
            >
              <svg
                width="15"
                height="15"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M8 2.4a3.6 3.6 0 0 1 3.6 3.6c0 2.5.6 3.5 1.1 4.1a.3.3 0 0 1-.2.5H3.5a.3.3 0 0 1-.2-.5c.5-.6 1.1-1.6 1.1-4.1A3.6 3.6 0 0 1 8 2.4Z" />
                <path d="M8 1.2v1.2" />
                <path d="M6.6 12.6a1.4 1.4 0 0 0 2.8 0" />
              </svg>
              {pendingOffers.length > 0 && (
                <span
                  style={{
                    position: "absolute",
                    top: -6,
                    right: -6,
                    minWidth: 16,
                    height: 16,
                    borderRadius: 20,
                    background: "var(--red)",
                    color: "#fff",
                    fontSize: 9,
                    fontWeight: 700,
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    padding: "0 4px",
                  }}
                  className="mono"
                >
                  {pendingOffers.length}
                </span>
              )}
            </button>
          </div>
        </div>

        {actionError && (
          <div
            role="alert"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "7px 18px",
              background: "#fdecec",
              borderBottom: "1px solid #f5c2c2",
              fontSize: 11.5,
              color: "#b91c1c",
              flex: "none",
            }}
          >
            <span>⚠</span>
            <span className="selectable" style={{ flex: 1, minWidth: 0 }}>
              {actionError}
            </span>
            <button
              onClick={dismissActionError}
              aria-label="Chiudi"
              style={{
                border: "none",
                background: "transparent",
                color: "#b91c1c",
                fontSize: 13,
                cursor: "pointer",
                padding: "0 4px",
              }}
            >
              ✕
            </button>
          </div>
        )}

        {loadError && (
          <div
            role="alert"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "7px 18px",
              background: "#fdecec",
              borderBottom: "1px solid #f5c2c2",
              fontSize: 11.5,
              color: "#b91c1c",
              flex: "none",
            }}
          >
            <span>⚠</span>
            <span className="selectable" style={{ flex: 1, minWidth: 0 }}>
              {loadError}
            </span>
            <button
              onClick={() => void reload()}
              style={{
                border: "1px solid rgba(185,28,28,.3)",
                background: "#fff",
                color: "#b91c1c",
                borderRadius: 7,
                padding: "3px 10px",
                fontSize: 11,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Riprova
            </button>
          </div>
        )}

        {versionMismatch && (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "7px 18px",
              background: "#fdf3e3",
              borderBottom: "1px solid #f0dfc0",
              fontSize: 11.5,
              color: "#8a5a1e",
              flex: "none",
            }}
          >
            <span style={{ flex: 1 }}>
              ⚠ Il daemon in esecuzione è la versione{" "}
              <b>{status?.version || "precedente"}</b>, questa app è la {guiVersion}.
            </span>
            <button
              onClick={() => void restartDaemon().catch(() => {})}
              style={{
                border: "1px solid rgba(138,90,30,.35)",
                background: "#fff",
                color: "#8a5a1e",
                borderRadius: 7,
                padding: "3px 10px",
                fontSize: 11,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Riavvia il daemon
            </button>
          </div>
        )}

        {boardView ? (
          <>
            {/* Quick-send drop zone. */}
            <div
              onClick={pick}
              onDragEnter={(e) => {
                e.preventDefault();
                setDragging(true);
              }}
              onDragOver={(e) => e.preventDefault()}
              onDragLeave={(e) => {
                e.preventDefault();
                setDragging(false);
              }}
              style={{
                margin: "4px 18px 4px",
                borderRadius: 14,
                border: `2px dashed ${dragging ? "var(--out)" : "#e2ddd6"}`,
                background: dragging ? "rgba(249,115,22,.08)" : "#fbf9f7",
                display: "flex",
                alignItems: "center",
                gap: 14,
                padding: "13px 18px",
                cursor: "pointer",
                transition: ".12s",
              }}
            >
              <div
                style={{
                  width: 38,
                  height: 38,
                  borderRadius: 11,
                  background: "var(--out)",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  color: "#fff",
                  fontSize: 19,
                  flex: "none",
                }}
              >
                ↑
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13.5, fontWeight: 600 }}>
                  {dragging
                    ? "Rilascia per scegliere il destinatario"
                    : "Trascina qui un file per inviarlo"}
                </div>
                <div style={{ fontSize: 11.5, color: "var(--ink-mut)" }}>
                  Trascina più file insieme per inviarli come gruppo · poi scegli
                  persona, ID/QR, codice, link o ticket.
                </div>
              </div>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  pick();
                }}
                style={{
                  flex: "none",
                  border: "none",
                  background: "var(--ink)",
                  color: "#fff",
                  borderRadius: 9,
                  padding: "9px 16px",
                  fontSize: 12.5,
                  fontWeight: 600,
                  cursor: "pointer",
                }}
              >
                Scegli file
              </button>
            </div>

            <ControlRow />

            <Board />
          </>
        ) : (
          <div
            style={{
              flex: 1,
              minHeight: 0,
              display: "flex",
              padding: "0 18px 14px",
            }}
          >
            {depositsOpen && <DepositsPanel />}
            {historyOpen && <HistoryPanel />}
            {contactsOpen && <ContactsPanel />}
          </div>
        )}
      </div>

      <SendSheet />
      <IncomingModal />
      <ReceiveModal />
    </div>
  );
}

function ControlRow() {
  const search = useStore((s) => s.search);
  const setSearch = useStore((s) => s.setSearch);
  const pauseAll = useStore((s) => s.pauseAll);
  const togglePauseAll = useStore((s) => s.togglePauseAll);
  const clearFinished = useStore((s) => s.clearFinished);
  // Offer the sweep only when there is something to sweep.
  const transfers = useStore((s) => s.transfers);
  const hasFinished = Object.values(transfers).some(
    (t) =>
      t.status === "completato" ||
      t.status === "fallito" ||
      t.status === "annullato"
  );
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "6px 18px 2px",
        flex: "none",
      }}
    >
      <div
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          gap: 8,
          background: "#fff",
          border: "1px solid var(--line-strong)",
          borderRadius: 10,
          padding: "8px 12px",
        }}
      >
        <span style={{ fontSize: 13, color: "var(--ink-mut)" }}>⌕</span>
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Cerca file o persona…"
          style={{
            flex: 1,
            border: "none",
            outline: "none",
            background: "transparent",
            fontSize: 12.5,
            color: "var(--ink)",
          }}
        />
      </div>
      <button
        onClick={togglePauseAll}
        style={{
          flex: "none",
          display: "flex",
          alignItems: "center",
          gap: 7,
          border: "1px solid var(--line-strong)",
          background: "#fff",
          borderRadius: 10,
          padding: "9px 14px",
          fontSize: 12,
          fontWeight: 600,
          cursor: "pointer",
        }}
      >
        <span style={{ fontSize: 11 }}>{pauseAll ? "▶" : "⏸"}</span>
        {pauseAll ? "Riprendi tutto" : "Pausa tutto"}
      </button>
      {hasFinished && (
        <button
          onClick={() => void clearFinished().catch(() => {})}
          title="Togli dalla board completati, falliti e annullati (lo storico li ricorda)"
          style={{
            flex: "none",
            border: "1px solid var(--line-strong)",
            background: "#fff",
            borderRadius: 10,
            padding: "9px 14px",
            fontSize: 12,
            fontWeight: 600,
            cursor: "pointer",
            color: "var(--ink-sec)",
          }}
        >
          Pulisci
        </button>
      )}
    </div>
  );
}

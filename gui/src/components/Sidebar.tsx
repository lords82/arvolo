// The app's spine: the two verbs you *do* on top (Invia, Ricevi), the four
// places you *look* below (Trasferimenti, Storico, Rubrica, Depositi), and who
// you are at the bottom. The grouping deliberately mirrors the CLI's own
// `--help` — "get files to someone / from someone" are the buttons, "follow it"
// and "people" are the views — so the two frontends teach each other.

import { useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import { shortId } from "../format";

type View = "board" | "history" | "contacts" | "deposits";

export function Sidebar() {
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const openHistory = useStore((s) => s.openHistory);
  const openContacts = useStore((s) => s.openContacts);
  const openDeposits = useStore((s) => s.openDeposits);
  const closeHistory = useStore((s) => s.closeHistory);
  const closeContacts = useStore((s) => s.closeContacts);
  const closeDeposits = useStore((s) => s.closeDeposits);

  const historyOpen = useStore((s) => s.historyOpen);
  const contactsOpen = useStore((s) => s.contactsOpen);
  const depositsOpen = useStore((s) => s.depositsOpen);
  const connected = useStore((s) => s.connected);
  const status = useStore((s) => s.status);
  const transfers = useStore((s) => s.transfers);

  const [copied, setCopied] = useState(false);

  const view: View = depositsOpen
    ? "deposits"
    : historyOpen
      ? "history"
      : contactsOpen
        ? "contacts"
        : "board";

  // Badge on Trasferimenti: offers still waiting for a yes/no.
  const pending = useMemo(
    () =>
      Object.values(transfers).filter((t) => t.status === "in arrivo").length,
    [transfers]
  );

  const goto = (v: View) => {
    // Turning every other flag off *is* the navigation; the store keeps them
    // exclusive, this just picks which one is on.
    if (v === "board") {
      closeHistory();
      closeContacts();
      closeDeposits();
    } else if (v === "history") void openHistory();
    else if (v === "contacts") openContacts();
    else void openDeposits();
  };

  const pick = async () => {
    const sel = await open({ multiple: true, directory: false });
    if (!sel) return;
    openSheet(Array.isArray(sel) ? sel : [sel]);
  };

  const myCode = status?.public_id ?? "";
  const myName = status?.display_name ?? "";

  return (
    <div
      style={{
        width: 190,
        flex: "none",
        display: "flex",
        flexDirection: "column",
        gap: 4,
        padding: "14px 10px 12px",
        borderRight: "1px solid var(--line)",
        background: "var(--card)",
      }}
    >
      {/* brand */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 6px 12px",
        }}
      >
        <div
          style={{
            width: 24,
            height: 24,
            borderRadius: 7,
            background: "var(--out)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "#fff",
            fontWeight: 700,
            fontSize: 14,
          }}
        >
          A
        </div>
        <span style={{ fontSize: 15, fontWeight: 700 }}>Arvolo</span>
      </div>

      {/* the two verbs */}
      <button
        onClick={() => void pick()}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          border: "none",
          background: "var(--out)",
          color: "#fff",
          borderRadius: 10,
          padding: "10px 12px",
          fontSize: 13,
          fontWeight: 700,
          cursor: "pointer",
        }}
      >
        <span style={{ fontSize: 14 }}>↗</span> Invia…
      </button>
      <button
        onClick={openReceive}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          border: "none",
          background: "var(--in)",
          color: "#fff",
          borderRadius: 10,
          padding: "10px 12px",
          fontSize: 13,
          fontWeight: 700,
          cursor: "pointer",
          marginBottom: 10,
        }}
      >
        <span style={{ fontSize: 14 }}>↙</span> Ricevi…
      </button>

      {/* the places you look */}
      <NavItem
        label="Trasferimenti"
        glyph="⇄"
        active={view === "board"}
        badge={pending || undefined}
        onClick={() => goto("board")}
      />
      <NavItem
        label="Storico"
        glyph="🕘"
        active={view === "history"}
        onClick={() => goto("history")}
      />
      <NavItem
        label="Rubrica"
        glyph="👥"
        active={view === "contacts"}
        onClick={() => goto("contacts")}
      />
      <NavItem
        label="Depositi"
        glyph="🔗"
        active={view === "deposits"}
        onClick={() => goto("deposits")}
      />

      <div style={{ flex: 1 }} />

      {/* who you are + how it's going */}
      <div
        style={{
          borderTop: "1px solid var(--line)",
          paddingTop: 10,
          display: "flex",
          flexDirection: "column",
          gap: 7,
        }}
      >
        {myCode && (
          <button
            onClick={() => {
              navigator.clipboard.writeText(myCode);
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            }}
            title="Copia il tuo codice"
            style={{
              display: "flex",
              flexDirection: "column",
              alignItems: "flex-start",
              gap: 2,
              background: copied ? "var(--teal-bg)" : "#f4f1ee",
              border: "none",
              borderRadius: 9,
              padding: "7px 10px",
              cursor: "pointer",
              textAlign: "left",
            }}
          >
            {myName && (
              <span style={{ fontSize: 11.5, fontWeight: 600 }}>{myName}</span>
            )}
            <span
              className="mono"
              style={{
                fontSize: 10,
                color: copied ? "var(--green)" : "var(--ink-mut)",
              }}
            >
              {copied ? "copiato ✓" : shortId(myCode)}
            </span>
          </button>
        )}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            padding: "0 4px",
          }}
        >
          <span
            style={{
              width: 7,
              height: 7,
              borderRadius: "50%",
              background: connected ? "var(--green)" : "var(--red)",
              flex: "none",
            }}
          />
          <span
            style={{
              fontSize: 10.5,
              fontWeight: 500,
              color: connected ? "var(--green)" : "var(--red)",
            }}
          >
            {connected ? "Connesso" : "Disconnesso"}
          </span>
          <span
            title="Cifratura end-to-end attiva"
            style={{
              marginLeft: "auto",
              fontSize: 8.5,
              fontWeight: 600,
              background: "var(--teal-bg)",
              color: "var(--teal)",
              padding: "2px 6px",
              borderRadius: 20,
            }}
          >
            E2E
          </span>
        </div>
      </div>
    </div>
  );
}

function NavItem({
  label,
  glyph,
  active,
  badge,
  onClick,
}: {
  label: string;
  glyph: string;
  active: boolean;
  badge?: number;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 9,
        border: "none",
        background: active ? "#f4f1ee" : "transparent",
        color: active ? "var(--ink)" : "var(--ink-sec)",
        borderRadius: 9,
        padding: "9px 12px",
        fontSize: 12.5,
        fontWeight: active ? 700 : 500,
        cursor: "pointer",
        textAlign: "left",
      }}
    >
      <span style={{ fontSize: 13, width: 16, textAlign: "center" }}>{glyph}</span>
      <span style={{ flex: 1 }}>{label}</span>
      {badge !== undefined && (
        <span
          className="mono"
          style={{
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
        >
          {badge}
        </span>
      )}
    </button>
  );
}

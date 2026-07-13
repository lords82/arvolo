import { useEffect, useMemo, useState } from "react";
import QRCode from "qrcode";
import { useStore } from "../store";
import { extOf, shortId } from "../format";

type Tab = "contatti" | "id" | "link" | "ticket";

const AVATAR_COLORS = [
  "#c2410c",
  "#0369a1",
  "#7c3aed",
  "#be185d",
  "#0f766e",
  "#4b5563",
];
function colorFor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}
function initials(name: string): string {
  const parts = name.trim().split(/\s+/);
  return (
    (parts[0]?.[0] ?? "?") + (parts[1]?.[0] ?? "")
  ).toUpperCase();
}

export function SendSheet() {
  const paths = useStore((s) => s.sheetPaths);
  const closeSheet = useStore((s) => s.closeSheet);
  const contacts = useStore((s) => s.contacts);
  const status = useStore((s) => s.status);
  const doSend = useStore((s) => s.send);
  const doTicket = useStore((s) => s.ticket);
  const doLink = useStore((s) => s.link);

  const [tab, setTab] = useState<Tab>("contatti");
  const [note, setNote] = useState("");
  const [code, setCode] = useState("");
  const [link, setLink] = useState("");
  const [ticket, setTicket] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  // Reset transient state whenever a new sheet opens.
  useEffect(() => {
    if (paths) {
      setTab("contatti");
      setNote("");
      setCode("");
      setLink("");
      setTicket("");
      setErr("");
      setBusy(false);
    }
  }, [paths]);

  const name = useMemo(() => {
    if (!paths || !paths.length) return "";
    const base = paths[0].split(/[\\/]/).pop() ?? paths[0];
    return base;
  }, [paths]);

  if (!paths) return null;

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setErr("");
    try {
      await fn();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const sizeLabel = paths.length > 1 ? `${paths.length} file` : "";

  return (
    <div
      onClick={closeSheet}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(20,16,12,.4)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        backdropFilter: "blur(2px)",
        zIndex: 100,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 520,
          maxHeight: 580,
          background: "#fff",
          borderRadius: 18,
          boxShadow: "0 30px 70px -12px rgba(0,0,0,.45)",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          animation: "pop .14s ease",
        }}
      >
        {/* header */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            padding: "18px 20px",
            borderBottom: "1px solid var(--line)",
          }}
        >
          <div
            className="mono"
            style={{
              width: 36,
              height: 36,
              borderRadius: 10,
              background: "#fff3e9",
              color: "#c2410c",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 9,
              fontWeight: 700,
              flex: "none",
            }}
          >
            {extOf(name)}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 13, color: "#a8a29a" }}>Invia</div>
            <div
              style={{
                fontSize: 15,
                fontWeight: 700,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {name}{" "}
              {sizeLabel && (
                <span style={{ fontWeight: 500, color: "#a8a29a", fontSize: 12 }}>
                  {sizeLabel}
                </span>
              )}
            </div>
          </div>
          <button
            onClick={closeSheet}
            style={{
              width: 30,
              height: 30,
              border: "none",
              background: "#f4f1ee",
              borderRadius: 8,
              cursor: "pointer",
              fontSize: 14,
            }}
          >
            ✕
          </button>
        </div>

        {/* tabs */}
        <div style={{ display: "flex", gap: 4, padding: "12px 16px 0" }}>
          {(
            [
              ["contatti", "Persone"],
              ["id", "ID / QR"],
              ["link", "Link"],
              ["ticket", "Ticket"],
            ] as [Tab, string][]
          ).map(([key, label]) => {
            const active = tab === key;
            return (
              <button
                key={key}
                onClick={() => setTab(key)}
                style={{
                  flex: 1,
                  border: "none",
                  borderRadius: 9,
                  padding: "9px 6px",
                  fontSize: 12,
                  fontWeight: 600,
                  cursor: "pointer",
                  background: active ? "#171514" : "#f4f1ee",
                  color: active ? "#fff" : "#57534c",
                }}
              >
                {label}
              </button>
            );
          })}
        </div>

        <div style={{ padding: "16px 20px 22px", overflowY: "auto" }}>
          {err && (
            <div
              style={{
                background: "#fdecec",
                color: "#b91c1c",
                borderRadius: 10,
                padding: "9px 12px",
                fontSize: 12,
                marginBottom: 12,
              }}
            >
              {err}
            </div>
          )}

          {(tab === "contatti" || tab === "id") && (
            <input
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="Aggiungi un messaggio (opzionale) — viaggia cifrato con il file"
              style={{
                width: "100%",
                border: "1px solid var(--line-strong)",
                borderRadius: 10,
                padding: "10px 12px",
                fontSize: 12,
                marginBottom: 14,
                outline: "none",
              }}
            />
          )}

          {tab === "contatti" && (
            <ContactsGrid
              onSend={(to) => run(async () => void (await doSend(to, paths, note)))}
              busy={busy}
              contacts={contacts}
            />
          )}

          {tab === "id" && (
            <IdTab
              code={code}
              setCode={setCode}
              myCode={status?.public_id ?? ""}
              busy={busy}
              onSend={() =>
                run(async () => void (await doSend(code.trim(), paths, note)))
              }
            />
          )}

          {tab === "link" && (
            <LinkTab
              link={link}
              busy={busy}
              multi={paths.length > 1}
              onCreate={() =>
                run(async () => {
                  const url = await doLink(paths[0]);
                  setLink(url);
                })
              }
            />
          )}

          {tab === "ticket" && (
            <TicketTab
              ticket={ticket}
              busy={busy}
              onCreate={() =>
                run(async () => {
                  const r = await doTicket(paths);
                  setTicket(r.ticket);
                })
              }
            />
          )}
        </div>
      </div>
    </div>
  );
}

function ContactsGrid({
  contacts,
  onSend,
  busy,
}: {
  contacts: { name: string; id: string; verified: boolean }[];
  onSend: (to: string) => void;
  busy: boolean;
}) {
  if (!contacts.length) {
    return (
      <div style={{ fontSize: 12.5, color: "#a8a29a", padding: "8px 2px" }}>
        Nessun contatto in rubrica. Aggiungine uno dalla CLI
        (<span className="mono">arvolo contacts add</span>) oppure invia a un ID
        dalla scheda “ID / QR”.
      </div>
    );
  }
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(3,1fr)",
        gap: 10,
      }}
    >
      {contacts.map((c) => (
        <button
          key={c.id}
          disabled={busy}
          onClick={() => onSend(c.name)}
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 8,
            padding: "16px 8px",
            border: "1px solid var(--line)",
            borderRadius: 14,
            background: "#fff",
            cursor: busy ? "default" : "pointer",
          }}
        >
          <div
            className="avatar"
            style={{
              width: 46,
              height: 46,
              background: colorFor(c.name),
              fontSize: 16,
              position: "relative",
            }}
          >
            {initials(c.name)}
            {c.verified && (
              <span
                style={{
                  position: "absolute",
                  bottom: 0,
                  right: 0,
                  width: 14,
                  height: 14,
                  borderRadius: "50%",
                  background: "#0f766e",
                  border: "2px solid #fff",
                  color: "#fff",
                  fontSize: 8,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                }}
              >
                ✓
              </span>
            )}
          </div>
          <span style={{ fontSize: 12.5, fontWeight: 600 }}>{c.name}</span>
          <span style={{ fontSize: 10.5, color: "#a8a29a" }}>
            {c.verified ? "verificato" : "non verificato"}
          </span>
        </button>
      ))}
    </div>
  );
}

function IdTab({
  code,
  setCode,
  myCode,
  onSend,
  busy,
}: {
  code: string;
  setCode: (v: string) => void;
  myCode: string;
  onSend: () => void;
  busy: boolean;
}) {
  const [qr, setQr] = useState("");
  useEffect(() => {
    if (myCode)
      QRCode.toDataURL(myCode, { margin: 1, width: 200 })
        .then(setQr)
        .catch(() => setQr(""));
  }, [myCode]);

  return (
    <div>
      <div style={{ fontSize: 12.5, color: "#57534c", marginBottom: 8 }}>
        Incolla il codice del destinatario per inviare.
      </div>
      <input
        value={code}
        onChange={(e) => setCode(e.target.value)}
        placeholder="if2xmnescalwohxlex5qylevzs2cypwdnjxe7sxb76wcphc7daha"
        className="mono"
        style={{
          width: "100%",
          border: "1px solid var(--line-strong)",
          borderRadius: 10,
          padding: "12px 14px",
          fontSize: 12,
          marginBottom: 12,
          outline: "none",
        }}
      />
      <button
        disabled={busy || !code.trim()}
        onClick={onSend}
        style={{
          width: "100%",
          border: "none",
          background: code.trim() ? "#f97316" : "#e2ddd6",
          color: "#fff",
          borderRadius: 11,
          padding: 12,
          fontSize: 13,
          fontWeight: 700,
          cursor: code.trim() ? "pointer" : "default",
          marginBottom: 18,
        }}
      >
        Invia a questo ID
      </button>

      {myCode && (
        <div style={{ display: "flex", gap: 14, alignItems: "center" }}>
          {qr && (
            <img
              src={qr}
              width={96}
              height={96}
              style={{ borderRadius: 12, flex: "none" }}
              alt="Il tuo QR"
            />
          )}
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>
              Il tuo codice
            </div>
            <div style={{ fontSize: 11.5, color: "#a8a29a", lineHeight: 1.5 }}>
              Fallo inquadrare a chi vuole inviarti file — oppure copialo dalla
              barra in alto. La scansione con fotocamera arriverà più avanti.
            </div>
            <div
              className="mono"
              style={{ fontSize: 11, color: "#57534c", marginTop: 6 }}
            >
              {shortId(myCode)}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function LinkTab({
  link,
  onCreate,
  busy,
  multi,
}: {
  link: string;
  onCreate: () => void;
  busy: boolean;
  multi: boolean;
}) {
  return (
    <div>
      <div style={{ fontSize: 12.5, color: "#57534c", marginBottom: 14 }}>
        Crea un URL scaricabile: chiunque abbia il link scarica il file dal
        browser, anche senza Arvolo. La chiave resta nel frammento
        <span className="mono"> #…</span> del link — il relay vede solo cifrato.
      </div>
      {multi && (
        <div style={{ fontSize: 11.5, color: "#b45309", marginBottom: 10 }}>
          ⚠ Hai selezionato più file: il link userà il primo. Per inviarli tutti
          insieme usa Ticket o Persone.
        </div>
      )}
      {link ? (
        <div>
          <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
            <input
              readOnly
              value={link}
              className="mono"
              style={{
                flex: 1,
                minWidth: 0,
                border: "1px solid var(--line-strong)",
                borderRadius: 10,
                padding: "11px 13px",
                fontSize: 12,
                background: "#faf8f6",
              }}
            />
            <button
              onClick={() => navigator.clipboard.writeText(link)}
              style={{
                border: "none",
                background: "#171514",
                color: "#fff",
                borderRadius: 10,
                padding: "0 16px",
                fontSize: 12,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Copia
            </button>
          </div>
          <div style={{ fontSize: 11.5, color: "#a8a29a" }}>
            Chiunque abbia questo link può scaricarlo. Revocalo dalla CLI con
            <span className="mono"> arvolo deposits</span>.
          </div>
        </div>
      ) : (
        <button
          disabled={busy}
          onClick={onCreate}
          style={primaryBtn(busy)}
        >
          {busy ? "Creazione…" : "Crea link di download"}
        </button>
      )}
    </div>
  );
}

function TicketTab({
  ticket,
  onCreate,
  busy,
}: {
  ticket: string;
  onCreate: () => void;
  busy: boolean;
}) {
  const [qr, setQr] = useState("");
  const [showQr, setShowQr] = useState(false);
  useEffect(() => {
    if (ticket)
      QRCode.toDataURL(ticket, { margin: 1, width: 260 })
        .then(setQr)
        .catch(() => setQr(""));
    else {
      setQr("");
      setShowQr(false);
    }
  }, [ticket]);

  return (
    <div>
      <div style={{ fontSize: 12.5, color: "#57534c", marginBottom: 14 }}>
        Genera un <b>ticket</b> — come un magnet link. Chi ce l'ha scarica in P2P
        direttamente da te (o dai peer), senza passare da un server.
      </div>
      {ticket ? (
        <div>
          <div
            style={{
              background: "#16181d",
              borderRadius: 12,
              padding: 14,
              marginBottom: 12,
            }}
          >
            <div
              className="mono"
              style={{
                fontSize: 11,
                color: "#34d399",
                wordBreak: "break-all",
                lineHeight: 1.5,
              }}
            >
              {ticket}
            </div>
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              onClick={() => navigator.clipboard.writeText(ticket)}
              style={{
                flex: 1,
                border: "none",
                background: "#171514",
                color: "#fff",
                borderRadius: 10,
                padding: 11,
                fontSize: 12,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              Copia ticket
            </button>
            <button
              onClick={() => setShowQr((v) => !v)}
              disabled={!qr}
              style={{
                flex: 1,
                border: "1px solid rgba(0,0,0,.14)",
                background: "#fff",
                borderRadius: 10,
                padding: 11,
                fontSize: 12,
                fontWeight: 600,
                cursor: qr ? "pointer" : "default",
              }}
            >
              {showQr ? "Nascondi QR" : "Mostra QR"}
            </button>
          </div>
          {showQr && qr && (
            <div style={{ display: "flex", justifyContent: "center", marginTop: 12 }}>
              <img
                src={qr}
                width={200}
                height={200}
                style={{ borderRadius: 12 }}
                alt="QR del ticket"
              />
            </div>
          )}
          <div style={{ fontSize: 11.5, color: "#a8a29a", marginTop: 10 }}>
            Il tuo dispositivo deve restare online finché il primo peer completa
            il download.
          </div>
        </div>
      ) : (
        <button disabled={busy} onClick={onCreate} style={primaryBtn(busy)}>
          {busy ? "Generazione…" : "Genera ticket P2P"}
        </button>
      )}
    </div>
  );
}

function primaryBtn(busy: boolean): React.CSSProperties {
  return {
    width: "100%",
    border: "none",
    background: busy ? "#e2ddd6" : "#f97316",
    color: "#fff",
    borderRadius: 11,
    padding: 13,
    fontSize: 13,
    fontWeight: 700,
    cursor: busy ? "default" : "pointer",
  };
}

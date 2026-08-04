import { useEffect, useMemo, useRef, useState } from "react";
import QRCode from "qrcode";
import { useStore } from "../store";
import { extOf, shortId } from "../format";

type Tab = "contatti" | "id" | "code" | "link" | "ticket";

const AVATAR_COLORS = [
  "#c2410c",
  "var(--in)",
  "#7c3aed",
  "#be185d",
  "var(--teal)",
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
  const doCode = useStore((s) => s.code);
  const doLink = useStore((s) => s.link);
  const openDeposits = useStore((s) => s.openDeposits);

  const [tab, setTab] = useState<Tab>("contatti");
  const [note, setNote] = useState("");
  const [code, setCode] = useState("");
  const [pairCode, setPairCode] = useState("");
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
      setPairCode("");
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

  // Close on a click that *began* on the backdrop, not on any click that merely
  // ends there. A file drop opens this sheet from under the pointer, and the click
  // closing the drag then lands on a backdrop that did not exist when the gesture
  // started — closing the sheet in the same frame it appeared. (It also stops a
  // text selection dragged out of the panel from dismissing it on release.)
  //
  // Must be declared *above* the early return: hooks have to run in the same order
  // on every render, and this component renders both closed (no paths) and open.
  // Below it, dropping a file added a hook mid-life and React tore the tree down.
  const pressedBackdrop = useRef(false);

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
      onMouseDown={(e) => {
        pressedBackdrop.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget && pressedBackdrop.current) closeSheet();
        pressedBackdrop.current = false;
      }}
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
            <div style={{ fontSize: 13, color: "var(--ink-mut)" }}>Invia</div>
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
                <span style={{ fontWeight: 500, color: "var(--ink-mut)", fontSize: 12 }}>
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
              ["code", "Codice"],
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
                  background: active ? "var(--ink)" : "#f4f1ee",
                  color: active ? "#fff" : "var(--ink-sec)",
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

          {tab === "code" && (
            <CodeTab
              code={pairCode}
              busy={busy}
              onCreate={(keep) =>
                run(async () => {
                  const r = await doCode(paths, keep);
                  setPairCode(r.code);
                })
              }
            />
          )}

          {tab === "link" && (
            <LinkTab
              link={link}
              busy={busy}
              multi={paths.length > 1}
              onCreate={(ttl, max) =>
                run(async () => {
                  const url = await doLink(paths[0], ttl, max);
                  setLink(url);
                })
              }
              onManage={() => {
                closeSheet();
                void openDeposits();
              }}
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
      <div style={{ fontSize: 12.5, color: "var(--ink-mut)", padding: "8px 2px" }}>
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
                  background: "var(--teal)",
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
          <span style={{ fontSize: 10.5, color: "var(--ink-mut)" }}>
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
      <div style={{ fontSize: 12.5, color: "var(--ink-sec)", marginBottom: 8 }}>
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
          background: code.trim() ? "var(--out)" : "#e2ddd6",
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
            <div style={{ fontSize: 11.5, color: "var(--ink-mut)", lineHeight: 1.5 }}>
              Fallo inquadrare a chi vuole inviarti file — oppure copialo dalla
              barra in alto. La scansione con fotocamera arriverà più avanti.
            </div>
            <div
              className="mono"
              style={{ fontSize: 11, color: "var(--ink-sec)", marginTop: 6 }}
            >
              {shortId(myCode)}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** How long a deposited link lives on the relay, offered as choices rather than
 *  a raw seconds field (the CLI takes `--ttl`; these are its common values). */
const TTL_CHOICES: [string, number][] = [
  ["1 ora", 3600],
  ["1 giorno", 24 * 3600],
  ["7 giorni", 7 * 24 * 3600],
  ["30 giorni", 30 * 24 * 3600],
];

function CodeTab({
  code,
  onCreate,
  busy,
}: {
  code: string;
  onCreate: (keep: boolean) => void;
  busy: boolean;
}) {
  const [keep, setKeep] = useState(false);
  const [qr, setQr] = useState("");
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (code)
      QRCode.toDataURL(code, { margin: 1, width: 200 })
        .then(setQr)
        .catch(() => setQr(""));
    else setQr("");
  }, [code]);

  return (
    <div>
      <div style={{ fontSize: 12.5, color: "var(--ink-sec)", marginBottom: 14 }}>
        Genera un <b>codice breve</b> tipo{" "}
        <span className="mono">4821-crater-mango</span>: si detta a voce o si
        scrive a mano, e chi lo riceve fa <span className="mono">arvolo recv
        &lt;codice&gt;</span> (o lo incolla in “Ricevi”). Il file viaggia comunque
        P2P — il relay fa solo da punto d'incontro. Il daemon lo tiene attivo
        anche se chiudi questa finestra.
      </div>
      {code ? (
        <div>
          <div
            style={{
              background: "#16181d",
              borderRadius: 12,
              padding: "18px 14px",
              marginBottom: 12,
              textAlign: "center",
            }}
          >
            <span
              className="mono selectable"
              style={{ fontSize: 22, fontWeight: 700, color: "#34d399", letterSpacing: ".04em" }}
            >
              {code}
            </span>
          </div>
          <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
            <button
              onClick={() => {
                navigator.clipboard.writeText(code);
                setCopied(true);
                setTimeout(() => setCopied(false), 1200);
              }}
              style={{
                flex: 1,
                border: "none",
                background: "var(--ink)",
                color: "#fff",
                borderRadius: 10,
                padding: 11,
                fontSize: 12,
                fontWeight: 600,
                cursor: "pointer",
              }}
            >
              {copied ? "Copiato ✓" : "Copia codice"}
            </button>
          </div>
          {qr && (
            <div style={{ display: "flex", justifyContent: "center", marginBottom: 10 }}>
              <img src={qr} width={160} height={160} style={{ borderRadius: 12 }} alt="QR del codice" />
            </div>
          )}
          <div style={{ fontSize: 11.5, color: "var(--ink-mut)" }}>
            Lo trovi anche sulla riga dell'invio nella board; interrompilo con
            “Annulla invio”.
          </div>
        </div>
      ) : (
        <div>
          <label
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              fontSize: 12,
              color: "var(--ink-sec)",
              marginBottom: 14,
              cursor: "pointer",
            }}
          >
            <input
              type="checkbox"
              checked={keep}
              onChange={(e) => setKeep(e.target.checked)}
            />
            Valido per più persone (finché non lo annulli)
          </label>
          {keep && (
            <div style={{ fontSize: 11, color: "var(--amber)", marginBottom: 12 }}>
              ⚠ Un codice riutilizzabile resta valido per chiunque lo intercetti,
              finché non lo annulli.
            </div>
          )}
          <button disabled={busy} onClick={() => onCreate(keep)} style={primaryBtn(busy)}>
            {busy ? "Generazione…" : "Genera codice"}
          </button>
        </div>
      )}
    </div>
  );
}

function LinkTab({
  link,
  onCreate,
  onManage,
  busy,
  multi,
}: {
  link: string;
  onCreate: (ttl: number | null, max: number | null) => void;
  /** Open the deposits panel — where the link can be watched and revoked. This
   *  used to be a line telling the user to go and run a CLI command (one that was
   *  never implemented, at that). */
  onManage: () => void;
  busy: boolean;
  multi: boolean;
}) {
  const [ttl, setTtl] = useState<number>(7 * 24 * 3600);
  const [max, setMax] = useState<string>("");
  return (
    <div>
      <div style={{ fontSize: 12.5, color: "var(--ink-sec)", marginBottom: 14 }}>
        Crea un URL scaricabile: chiunque abbia il link scarica il file dal
        browser, anche senza Arvolo. La chiave resta nel frammento
        <span className="mono"> #…</span> del link — il relay vede solo cifrato.
      </div>
      {multi && (
        <div style={{ fontSize: 11.5, color: "var(--amber)", marginBottom: 10 }}>
          ⚠ Hai selezionato più file: il link userà il primo. Per inviarli tutti
          insieme usa Ticket o Persone.
        </div>
      )}
      {!link && (
        <div style={{ display: "flex", gap: 10, marginBottom: 14 }}>
          <label style={{ flex: 1, fontSize: 11, color: "var(--ink-sec)" }}>
            Scade dopo
            <select
              value={ttl}
              onChange={(e) => setTtl(Number(e.target.value))}
              style={{
                display: "block",
                width: "100%",
                marginTop: 4,
                border: "1px solid var(--line-strong)",
                borderRadius: 8,
                padding: "8px 10px",
                fontSize: 12,
                background: "#fff",
              }}
            >
              {TTL_CHOICES.map(([label, secs]) => (
                <option key={secs} value={secs}>
                  {label}
                </option>
              ))}
            </select>
          </label>
          <label style={{ flex: 1, fontSize: 11, color: "var(--ink-sec)" }}>
            Max download (vuoto = senza limite)
            <input
              type="number"
              min={1}
              value={max}
              onChange={(e) => setMax(e.target.value)}
              placeholder="∞"
              style={{
                display: "block",
                width: "100%",
                marginTop: 4,
                border: "1px solid var(--line-strong)",
                borderRadius: 8,
                padding: "8px 10px",
                fontSize: 12,
              }}
            />
          </label>
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
                background: "var(--ink)",
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
          <div style={{ fontSize: 11.5, color: "var(--ink-mut)" }}>
            Chiunque abbia questo link può scaricarlo. Puoi revocarlo quando vuoi
            da{" "}
            <button
              onClick={onManage}
              style={{
                border: "none",
                background: "transparent",
                padding: 0,
                font: "inherit",
                color: "#7c3aed",
                fontWeight: 600,
                cursor: "pointer",
                textDecoration: "underline",
              }}
            >
              Link e depositi
            </button>
            .
          </div>
        </div>
      ) : (
        <button
          disabled={busy}
          onClick={() => onCreate(ttl, max.trim() ? Number(max) : null)}
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
      <div style={{ fontSize: 12.5, color: "var(--ink-sec)", marginBottom: 14 }}>
        Genera un <b>ticket</b>: chi ce l'ha scarica il file in P2P direttamente da
        te (o dai peer), senza passare da un server. Vale come una chiave —
        tienilo privato, chiunque l'abbia può aprire il file.
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
                background: "var(--ink)",
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
          <div style={{ fontSize: 11.5, color: "var(--ink-mut)", marginTop: 10 }}>
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
    background: busy ? "#e2ddd6" : "var(--out)",
    color: "#fff",
    borderRadius: 11,
    padding: 13,
    fontSize: 13,
    fontWeight: 700,
    cursor: busy ? "default" : "pointer",
  };
}

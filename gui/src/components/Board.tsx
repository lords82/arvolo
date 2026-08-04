import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "../store";
import {
  barColor,
  extOf,
  extTint,
  fmtBytes,
  methodMeta,
  metaLine,
  pct,
  sectionsFor,
  statusMeta,
  type Section,
} from "../format";
import type { UITransfer } from "../types";

export function Board() {
  const transfers = useStore((s) => s.transfers);
  const search = useStore((s) => s.search);
  const closeMenu = useStore((s) => s.toggleMenu);

  const rows = Object.values(transfers).sort((a, b) => b.rank - a.rank);
  const outSections = sectionsFor(rows, "out", search);
  const inSections = sectionsFor(rows, "in", search);

  // "In corso" = still open, which is the same set the row menu still offers to
  // cancel — a deposit awaiting pickup included: it is an outstanding send until
  // the recipient fetches it. Counting it as closed while still offering "Annulla"
  // was the inconsistency that made cancelling a deposit move no number at all.
  // A cancel in flight drops out immediately, matching the row's own optimistic
  // "Annullamento…"; if the daemon refuses, both come back together.
  // "In totale" stays every row in the column, concluded ones included.
  const count = (dir: "out" | "in") => {
    const f = rows.filter((t) => t.dir === dir);
    const open = f.filter(
      (t) =>
        t.status === "in corso" ||
        t.status === "in attesa" ||
        t.status === "in stallo" ||
        t.status === "deposited"
    ).length;
    return `${open} in corso · ${f.length} in totale`;
  };

  return (
    <div
      onClick={() => closeMenu(null)}
      style={{
        flex: 1,
        display: "flex",
        minHeight: 0,
        padding: "8px 12px 14px",
        gap: 12,
      }}
    >
      <Column
        title="Inviati"
        glyph="↗"
        glyphBg="#fff3e9"
        glyphColor="#c2410c"
        countLabel={count("out")}
        sections={outSections}
        emptyTitle="Nessun invio"
        emptyBody="Trascina un file qui sopra per inviarlo a una persona, un ID, un link o un ticket."
      />
      <Column
        title="Ricevuti"
        glyph="↙"
        glyphBg="#e9f3fb"
        glyphColor="var(--in)"
        countLabel={count("in")}
        sections={inSections}
        emptyTitle="Niente in arrivo"
        emptyBody="Condividi il tuo codice o QR: i file che ricevi appariranno qui."
      />
    </div>
  );
}

function Column(props: {
  title: string;
  glyph: string;
  glyphBg: string;
  glyphColor: string;
  countLabel: string;
  sections: Section[];
  emptyTitle: string;
  emptyBody: string;
}) {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        minWidth: 0,
        background: "#fff",
        border: "1px solid var(--line)",
        borderRadius: 16,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          padding: "14px 16px 10px",
          borderBottom: "1px solid var(--line)",
        }}
      >
        <span
          style={{
            width: 24,
            height: 24,
            borderRadius: 7,
            background: props.glyphBg,
            color: props.glyphColor,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 13,
            fontWeight: 700,
          }}
        >
          {props.glyph}
        </span>
        <span style={{ fontSize: 14, fontWeight: 700 }}>{props.title}</span>
        <span
          className="mono"
          style={{
            marginLeft: "auto",
            fontSize: 10.5,
            fontWeight: 500,
            color: "var(--ink-mut)",
          }}
        >
          {props.countLabel}
        </span>
      </div>

      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "8px 12px 12px",
          display: "flex",
          flexDirection: "column",
          gap: 6,
        }}
      >
        {props.sections.length === 0 ? (
          <Empty title={props.emptyTitle} body={props.emptyBody} />
        ) : (
          props.sections.map((sec) => (
            <div key={sec.key}>
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 7,
                  padding: "8px 4px 2px",
                }}
              >
                <span
                  className="mono"
                  style={{
                    fontSize: 9.5,
                    fontWeight: 600,
                    letterSpacing: ".08em",
                    textTransform: "uppercase",
                    color: "var(--ink-mut)",
                  }}
                >
                  {sec.title}
                </span>
                <span
                  className="mono"
                  style={{ fontSize: 9, fontWeight: 600, color: "var(--ink-weak)" }}
                >
                  {sec.items.length}
                </span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {sec.items.map((t) => (
                  <TransferRow key={t.key} t={t} />
                ))}
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function Empty({ title, body }: { title: string; body: string }) {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 10,
        padding: "30px 20px",
        textAlign: "center",
      }}
    >
      <div
        style={{
          width: 46,
          height: 46,
          borderRadius: 14,
          border: "2px dashed #e2ddd6",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 20,
          color: "var(--ink-weak)",
        }}
      >
        ⊘
      </div>
      <div style={{ fontSize: 13, fontWeight: 600, color: "var(--ink-sec)" }}>{title}</div>
      <div
        style={{
          fontSize: 11.5,
          color: "var(--ink-mut)",
          maxWidth: 210,
          lineHeight: 1.5,
        }}
      >
        {body}
      </div>
    </div>
  );
}

function Chip({
  bg,
  color,
  glyph,
  children,
}: {
  bg: string;
  color: string;
  glyph?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 3,
        fontSize: 9,
        fontWeight: 600,
        background: bg,
        color,
        padding: "2px 6px",
        borderRadius: 20,
      }}
    >
      {glyph && <span style={{ fontSize: 10 }}>{glyph}</span>}
      {children}
    </span>
  );
}

function TransferRow({ t }: { t: UITransfer }) {
  const store = useStore();
  const md = methodMeta(t.method);
  const tint = extTint(extOf(t.name));
  const sm = statusMeta(t.status);
  const showBar = t.status === "in corso" || t.status === "in stallo";
  const multi = t.swarmPeers > 1;
  const isPending = t.status === "in arrivo";

  const rowClick = () => {
    if (isPending && t.offerId) store.openIncoming(t.offerId);
  };

  return (
    <div
      onClick={rowClick}
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 11,
        padding: "11px 12px",
        border: "1px solid rgba(0,0,0,.07)",
        borderRadius: 12,
        cursor: isPending ? "pointer" : "default",
      }}
    >
      <div
        style={{
          width: 32,
          height: 32,
          borderRadius: 9,
          flex: "none",
          background: tint[0],
          color: tint[1],
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontSize: 8.5,
          fontWeight: 700,
          marginTop: 2,
        }}
        className="mono"
      >
        {extOf(t.name)}
      </div>

      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: 7 }}>
          <span
            style={{
              fontSize: 12.5,
              fontWeight: 600,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {t.name}
          </span>
          <span
            className="mono"
            style={{
              fontSize: 10.5,
              fontWeight: 500,
              color: "var(--ink-weak)",
              whiteSpace: "nowrap",
            }}
          >
            {t.files > 1 ? `${t.files} file · ` : ""}
            {fmtBytes(t.size)}
          </span>
        </div>

        {showBar && (
          <div className="prog" style={{ background: "#f0ece7", marginTop: 6 }}>
            <span style={{ width: `${pct(t)}%`, background: barColor(t) }} />
          </div>
        )}

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            marginTop: 5,
            flexWrap: "wrap",
          }}
        >
          <Chip bg={md.bg} color={md.color} glyph={md.glyph}>
            {md.label}
          </Chip>
          <Chip
            bg={t.encrypted ? "var(--teal-bg)" : "#f0ece7"}
            color={t.encrypted ? "var(--teal)" : "#8a827a"}
          >
            {t.encrypted ? "E2E" : "Pubblico"}
          </Chip>
          {t.note && (
            <Chip bg="#f3ead9" color="#8a6d3b" glyph="✉">
              messaggio
            </Chip>
          )}
          {t.code && (
            /* The live pairing code this send answers to — click to copy, since
               the whole point of a code is handing it to someone. */
            <button
              onClick={(e) => {
                e.stopPropagation();
                navigator.clipboard.writeText(t.code!);
              }}
              title="Codice attivo — clicca per copiarlo"
              className="mono"
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 3,
                fontSize: 9.5,
                fontWeight: 700,
                background: "#16181d",
                color: "#34d399",
                padding: "2px 7px",
                borderRadius: 20,
                border: "none",
                cursor: "pointer",
              }}
            >
              {t.code} ⧉
            </button>
          )}
          {t.downloadPeers > 0 && (
            <Chip bg="#eaf3ec" color="#2f7d4f" glyph="↥">
              servo {t.downloadPeers} peer
            </Chip>
          )}
          {multi ? (
            <Chip bg="#e9f3fb" color="var(--in)" glyph="⇄">
              {t.swarmPeers} peer
            </Chip>
          ) : (
            <span
              style={{
                fontSize: 10.5,
                color: "var(--ink-mut)",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {t.peer}
            </span>
          )}
          {t.verified && (
            <span style={{ fontSize: 9.5, fontWeight: 600, color: "var(--teal)" }}>
              ✓ verificato
            </span>
          )}
        </div>
      </div>

      <div
        style={{
          textAlign: "right",
          flex: "none",
          minWidth: 92,
          maxWidth: 150,
          marginTop: 2,
        }}
      >
        <div style={{ fontSize: 10.5, fontWeight: 600, color: sm.color }}>
          {sm.text}
        </div>
        <div
          className="mono"
          title={metaLine(t)}
          style={{
            fontSize: 9.5,
            fontWeight: 500,
            color: "var(--ink-mut)",
            marginTop: 2,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {metaLine(t)}
        </div>
      </div>

      {isPending && t.offerId && (
        <div style={{ display: "flex", gap: 6, flex: "none", marginTop: 2 }}>
          {/* The store surfaces failures in the error banner; the catch is only to
              keep a daemon refusal from becoming an unhandled rejection. */}
          <button
            onClick={(e) => {
              e.stopPropagation();
              void store.reject(t.offerId!).catch(() => {});
            }}
            style={btnGhost}
          >
            Rifiuta
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              void store.accept(t.offerId!, null).catch(() => {});
            }}
            style={btnAccept}
          >
            Accetta
          </button>
        </div>
      )}

      <RowMenu t={t} />
    </div>
  );
}

function RowMenu({ t }: { t: UITransfer }) {
  const store = useStore();
  const open = useStore((s) => s.openMenuKey === t.key);
  const contact = useStore((s) =>
    t.peerId ? s.contactsById[t.peerId] : undefined
  );

  interface Action {
    label: string;
    glyph: string;
    color: string;
    onClick: () => void;
  }
  const actions: Action[] = [];
  if (t.status === "in arrivo" && t.offerId) {
    actions.push({
      label: "Apri dettagli",
      glyph: "⤢",
      color: "var(--ink)",
      onClick: () => store.openIncoming(t.offerId!),
    });
    actions.push({
      label: "Accetta",
      glyph: "✓",
      color: "var(--green)",
      onClick: () => void store.accept(t.offerId!, null).catch(() => {}),
    });
    actions.push({
      label: "Rifiuta",
      glyph: "✕",
      color: "var(--red)",
      onClick: () => void store.reject(t.offerId!).catch(() => {}),
    });
  } else {
    // "Live" = still cancellable rather than merely removable. A Deposited send
    // counts: the file is sitting on the relay awaiting pickup, and cancelling it
    // withdraws it there (see TransferManager::cancel).
    const live =
      t.status === "in corso" ||
      t.status === "in attesa" ||
      t.status === "in stallo" ||
      t.status === "deposited";
    // A cancel already in flight: neither "Annulla" (it's happening) nor "Elimina"
    // (the daemon refuses to drop an unfinished transfer) applies until it lands.
    const cancelling = t.status === "in annullamento";
    if (t.status === "in corso" || t.status === "in stallo")
      actions.push({
        label: "Metti in pausa",
        glyph: "⏸",
        color: "var(--ink)",
        onClick: () => void store.pause(t.id).catch(() => {}),
      });
    if (t.status === "in attesa")
      actions.push({
        label: "Riprendi",
        glyph: "▶",
        color: "var(--ink)",
        onClick: () => void store.resume(t.id).catch(() => {}),
      });
    if (t.status === "completato" && t.path)
      actions.push({
        label: "Apri cartella",
        glyph: "⌖",
        color: "var(--ink)",
        onClick: () => {
          revealItemInDir(t.path!).catch(() => {});
          store.toggleMenu(null);
        },
      });
    // Only offered for a saved contact: the verified mark is keyed to the
    // address book. Routes to the Rubrica, where the fingerprint is shown and
    // confirmed — a one-click verify with no fingerprint on screen would make
    // the badge meaningless.
    if (!t.verified && contact)
      actions.push({
        label: "Verifica identità…",
        glyph: "✓",
        color: "var(--teal)",
        onClick: () => {
          store.toggleMenu(null);
          store.openContacts();
        },
      });
    actions.push({
      label: "Sposta su",
      glyph: "↑",
      color: "var(--ink)",
      onClick: () => store.moveItem(t.key, -1),
    });
    actions.push({
      label: "Sposta giù",
      glyph: "↓",
      color: "var(--ink)",
      onClick: () => store.moveItem(t.key, 1),
    });
    if (!cancelling)
      actions.push(
        live
          ? {
              label: t.dir === "out" ? "Annulla invio" : "Annulla",
              glyph: "✕",
              color: "var(--red)",
              onClick: () => void store.cancel(t.id).catch(() => {}),
            }
          : {
              label: "Elimina",
              glyph: "✕",
              color: "var(--red)",
              onClick: () => void store.removeRow(t.key).catch(() => {}),
            }
      );
  }

  return (
    <div style={{ position: "relative", flex: "none", marginTop: 2 }}>
      <button
        aria-label="Azioni trasferimento"
        onClick={(e) => {
          e.stopPropagation();
          store.toggleMenu(t.key);
        }}
        style={{
          width: 28,
          height: 28,
          border: "none",
          background: "transparent",
          borderRadius: 7,
          cursor: "pointer",
          fontSize: 16,
          color: "var(--ink-mut)",
          lineHeight: 1,
        }}
      >
        ⋮
      </button>
      {open && (
        <div
          onClick={(e) => e.stopPropagation()}
          style={{
            position: "absolute",
            top: 31,
            right: 0,
            background: "#fff",
            border: "1px solid var(--line-strong)",
            borderRadius: 11,
            boxShadow: "0 14px 34px -8px rgba(0,0,0,.3)",
            padding: 5,
            minWidth: 170,
            zIndex: 50,
            animation: "pop .12s ease",
          }}
        >
          {actions.map((a, i) => (
            <button
              key={i}
              onClick={a.onClick}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                width: "100%",
                border: "none",
                background: "transparent",
                borderRadius: 7,
                padding: "8px 10px",
                fontSize: 12,
                fontWeight: 500,
                color: a.color,
                cursor: "pointer",
                textAlign: "left",
              }}
            >
              <span style={{ fontSize: 12, width: 14, textAlign: "center" }}>
                {a.glyph}
              </span>
              {a.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

const btnGhost: React.CSSProperties = {
  border: "1px solid rgba(0,0,0,.14)",
  background: "#fff",
  borderRadius: 8,
  padding: "6px 10px",
  fontSize: 11,
  fontWeight: 600,
  cursor: "pointer",
};
const btnAccept: React.CSSProperties = {
  border: "none",
  background: "var(--green)",
  color: "#fff",
  borderRadius: 8,
  padding: "6px 12px",
  fontSize: 11,
  fontWeight: 700,
  cursor: "pointer",
};

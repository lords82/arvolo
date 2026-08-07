// The board: everything in flight, and everything recently finished.
//
// Two columns, one per direction, each grouped into the same four sections
// (`sectionsFor` in format.ts owns that grouping and the search filter). Two
// columns rather than one merged list because the two directions demand
// different things of you — an incoming offer is a decision, an outgoing send is
// a status — and interleaving them makes you re-read every row to find out which
// kind it is.
//
// On a narrow window the columns stack; the direction stripe down each row's
// leading edge is what keeps them distinguishable once they do.

import { useMemo, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { fire, useStore } from "../store";
import {
  fmtBytes,
  metaLine,
  sectionsFor,
  statusMeta,
  type Section,
} from "../format";
import type { UITransfer } from "../types";
import { Icon } from "../ui/Icons";
import { Button, Empty, IconButton } from "../ui/Primitives";
import { ExtChip, Progress } from "../ui/Bits";
import { MenuButton, type MenuItem } from "../ui/Menu";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";

function Row({ t }: { t: UITransfer }) {
  const pause = useStore((s) => s.pause);
  const resume = useStore((s) => s.resume);
  const cancel = useStore((s) => s.cancel);
  const removeRow = useStore((s) => s.removeRow);
  const openIncoming = useStore((s) => s.openIncoming);
  const moveItem = useStore((s) => s.moveItem);
  const [confirmCancel, setConfirmCancel] = useState(false);

  const st = statusMeta(t.status);
  const meta = metaLine(t);
  // The swarm is the whole point of the P2P path, and a row that is pulling from
  // three peers at once has no other way of saying so.
  const peers = Math.max(t.swarmPeers, t.downloadPeers);
  // `deposited` counts as live here: the blob is on the relay awaiting pickup
  // and can still be withdrawn. Leaving it out made every "Revoca il deposito"
  // string in this file unreachable, so the board offered no way to take one
  // back at all.
  const live =
    t.status === "in corso" ||
    t.status === "in attesa" ||
    t.status === "in stallo" ||
    t.status === "deposited" ||
    t.status === "in annullamento";
  const done =
    t.status === "completato" ||
    t.status === "annullato" ||
    t.status === "fallito";

  const items: MenuItem[] = [];
  if (t.status === "in corso") {
    items.push({
      key: "pause",
      label: "Metti in pausa",
      icon: <Icon.Pause size={13} />,
      onSelect: () => fire(pause(t.id)),
    });
  }
  if (t.status === "in attesa" || t.status === "in stallo") {
    items.push({
      key: "resume",
      label: "Riprendi",
      icon: <Icon.Play size={13} />,
      onSelect: () => fire(resume(t.id)),
    });
  }
  if (t.path) {
    items.push({
      key: "open",
      label: "Apri la cartella",
      icon: <Icon.Folder size={13} />,
      // `revealItemInDir` rather than opening the path: it shows the file
      // *selected* in the file manager, which is what someone who just received
      // something actually wants — opening its folder loses it among the rest.
      onSelect: () => {
        revealItemInDir(t.path!).catch((e: unknown) =>
          toast.bad("Non riesco ad aprire la cartella", String(e))
        );
      },
    });
  }
  items.push(
    {
      key: "up",
      label: "Sposta su",
      onSelect: () => moveItem(t.key, -1),
      separated: items.length > 0,
    },
    { key: "down", label: "Sposta giù", onSelect: () => moveItem(t.key, 1) }
  );
  if (live) {
    items.push({
      key: "cancel",
      label: t.status === "deposited" ? "Revoca il deposito" : "Annulla",
      icon: <Icon.Stop size={13} />,
      danger: true,
      separated: true,
      onSelect: () => setConfirmCancel(true),
    });
  }
  if (done) {
    items.push({
      key: "remove",
      label: "Togli dalla lista",
      icon: <Icon.Trash size={13} />,
      danger: true,
      separated: true,
      onSelect: () => fire(removeRow(t.key)),
    });
  }

  // A parked offer is a decision, not a status: the whole row opens the dialog.
  const isOffer = t.status === "in arrivo";

  return (
    <>
      <div
        className={`row dir-${t.dir} ${done ? "is-done" : ""}`}
        style={isOffer ? { cursor: "pointer" } : undefined}
        onClick={isOffer ? () => openIncoming(t.offerId!) : undefined}
        role={isOffer ? "button" : undefined}
        tabIndex={isOffer ? 0 : undefined}
        onKeyDown={
          isOffer
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openIncoming(t.offerId!);
                }
              }
            : undefined
        }
      >
        <ExtChip name={t.name} />

        <div className="row-main">
          <div className="row-name truncate" title={t.name}>
            {t.name}
          </div>
          <div className="row-meta">
            <span className={`tone-${st.tone}`} style={{ fontWeight: 600 }}>
              {st.text}
            </span>
            {t.peer && (
              <>
                <span className="sep" />
                <span className="truncate" title={t.peerId}>
                  {t.dir === "out" ? "a" : "da"} {t.peer}
                </span>
              </>
            )}
            {t.verified && (
              <Icon.Shield
                size={11}
                className="tone-ok"
                label="Identità verificata"
              />
            )}
            {t.size > 0 && (
              <>
                <span className="sep" />
                <span className="tnum">{fmtBytes(t.size)}</span>
              </>
            )}
            {meta && (
              <>
                <span className="sep" />
                <span className="truncate tnum">{meta}</span>
              </>
            )}
            {peers > 0 && (
              <>
                <span className="sep" />
                <span className="tnum" title="Trasferimento distribuito fra più peer">
                  ⇄ {peers} peer
                </span>
              </>
            )}
          </div>
          {(live || t.status === "completato") && <Progress t={t} />}
          {t.code && (
            <div className="hstack-sm" style={{ marginTop: 6 }}>
              <Icon.Qr size={12} className="t-mut" />
              <code className="mono t-xs">{t.code}</code>
              <span className="t-xs t-mut">codice attivo</span>
            </div>
          )}
        </div>

        <div className="row-actions" onClick={(e) => e.stopPropagation()}>
          {isOffer ? (
            <Button
              size="sm"
              variant="in"
              onClick={() => openIncoming(t.offerId!)}
            >
              Rivedi
            </Button>
          ) : (
            <>
              {t.status === "in corso" && (
                <IconButton label="Metti in pausa" onClick={() => fire(pause(t.id))}>
                  <Icon.Pause size={14} />
                </IconButton>
              )}
              {(t.status === "in attesa" || t.status === "in stallo") && (
                <IconButton label="Riprendi" onClick={() => fire(resume(t.id))}>
                  <Icon.Play size={14} />
                </IconButton>
              )}
              <MenuButton items={items} label={`Azioni per ${t.name}`}>
                <Icon.More size={15} />
              </MenuButton>
            </>
          )}
        </div>
      </div>

      <Confirm
        open={confirmCancel}
        title={t.status === "deposited" ? "Revocare il deposito?" : "Annullare?"}
        body={
          t.status === "deposited"
            ? `Il file viene rimosso dal relay e l'offerta ritirata dalla casella di ${t.peer ?? "destinazione"}. Non potrà più scaricarlo.`
            : `«${t.name}» si ferma qui. Quello che è già passato viene buttato: se lo rifai, riparte da capo.`
        }
        confirmLabel={
          t.status === "deposited" ? "Revoca" : "Annulla il trasferimento"
        }
        cancelLabel="Lascia stare"
        danger
        onCancel={() => setConfirmCancel(false)}
        onConfirm={() => {
          setConfirmCancel(false);
          fire(cancel(t.id));
        }}
      />
    </>
  );
}

function Column({
  dir,
  sections,
  title,
}: {
  dir: "out" | "in";
  sections: Section[];
  title: string;
}) {
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const total = sections.reduce((n, s) => n + s.items.length, 0);

  return (
    <section style={{ minWidth: 0 }}>
      <div className="section-head">
        <span className={`t-label tone-${dir === "out" ? "out" : "in"}`}>
          {dir === "out" ? <Icon.Send size={11} /> : <Icon.Receive size={11} />}{" "}
          {title}
        </span>
        <span className="t-xs t-mut tnum">{total}</span>
      </div>

      {total === 0 ? (
        <div className="card">
          <Empty
            icon={dir === "out" ? <Icon.Send size={22} /> : <Icon.Receive size={22} />}
            title={dir === "out" ? "Niente in uscita" : "Niente in arrivo"}
          >
            {dir === "out"
              ? "Trascina un file nella finestra, o usa Invia."
              : "Qui compaiono i file che qualcuno ti manda."}
            <div style={{ marginTop: 10 }}>
              <Button
                size="sm"
                onClick={() => (dir === "out" ? openSheet([]) : openReceive())}
              >
                {dir === "out" ? "Invia qualcosa" : "Incolla un codice"}
              </Button>
            </div>
          </Empty>
        </div>
      ) : (
        sections.map((sec) => (
          <div key={sec.key} className="section">
            <div className="section-head">
              <span className="t-label">{sec.title}</span>
              <span className="t-xs t-mut tnum">{sec.items.length}</span>
            </div>
            <div className="card rows">
              {sec.items
                .slice()
                .sort((a, b) => b.rank - a.rank)
                .map((t) => (
                  <Row key={t.key} t={t} />
                ))}
            </div>
          </div>
        ))
      )}
    </section>
  );
}

export function TransfersView() {
  const transfers = useStore((s) => s.transfers);
  const search = useStore((s) => s.search);
  const openSheet = useStore((s) => s.openSheet);

  const rows = useMemo(() => Object.values(transfers), [transfers]);
  const out = useMemo(() => sectionsFor(rows, "out", search), [rows, search]);
  const inc = useMemo(() => sectionsFor(rows, "in", search), [rows, search]);

  // A first-run window with nothing in it should say what to do, not show two
  // empty columns. Past that the columns carry their own empty states, which are
  // quieter and keep the layout stable as rows come and go.
  if (!rows.length) {
    return (
      <div className="view-narrow" style={{ paddingTop: 30 }}>
        <button className="dropzone" onClick={() => openSheet([])}>
          <Icon.Send size={30} className="tone-out" />
          <div className="t-head">Trascina qui i file da inviare</div>
          <div className="t-sm t-mut" style={{ maxWidth: "44ch" }}>
            Oppure scegli un contatto, genera un codice da leggere al volo, o
            crea un link che si apre in qualsiasi browser. Tutto è cifrato
            end-to-end: il relay vede solo byte illeggibili.
          </div>
          <span className="btn btn-primary" style={{ marginTop: 6 }}>
            <Icon.Send size={14} />
            Invia qualcosa
          </span>
        </button>
      </div>
    );
  }

  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(min(360px, 100%), 1fr))",
        gap: 24,
        alignItems: "start",
      }}
    >
      <Column dir="out" sections={out} title="In uscita" />
      <Column dir="in" sections={inc} title="In arrivo" />
    </div>
  );
}

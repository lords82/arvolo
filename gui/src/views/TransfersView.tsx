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

import { useCallback, useMemo, useState } from "react";
import type { CSSProperties } from "react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { fire, useStore } from "../store";
import {
  fmtBytes,
  metaLine,
  sectionsFor,
  statusMeta,
  type Section,
} from "../format";
import { useT } from "../i18n";
import type { UITransfer } from "../types";
import { Icon } from "../ui/Icons";
import { Button, Empty, IconButton } from "../ui/Primitives";
import { ExtChip, Progress } from "../ui/Bits";
import { MenuButton, type MenuItem } from "../ui/Menu";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import { useReorder, type ReorderBits } from "./reorder";

// `t` is the translator everywhere in this app, so the transfer this row draws
// is `tx`. The prop keeps its name — the callers read better as `t={row}`.
function Row({
  t: tx,
  grip,
  onNudge,
  drag,
}: {
  t: UITransfer;
  /** Handle wiring from `useReorder`, on the file-kind chip. */
  grip: ReturnType<ReorderBits["gripProps"]>;
  /** Keyboard equivalent of the drag: −1 up, 1 down. */
  onNudge: (delta: 1 | -1) => void;
  drag: { className: string; style?: CSSProperties };
}) {
  const t = useT();
  const pause = useStore((s) => s.pause);
  const resume = useStore((s) => s.resume);
  const cancel = useStore((s) => s.cancel);
  const removeRow = useStore((s) => s.removeRow);
  const openIncoming = useStore((s) => s.openIncoming);
  const openShare = useStore((s) => s.openShare);
  const [confirmCancel, setConfirmCancel] = useState(false);

  const st = statusMeta(tx.status);
  const meta = metaLine(tx);
  // The swarm is the whole point of the P2P path, and a row that is pulling from
  // three peers at once has no other way of saying so.
  const peers = Math.max(tx.swarmPeers, tx.downloadPeers);
  // `deposited` counts as live here: the blob is on the relay awaiting pickup
  // and can still be withdrawn. Leaving it out made every "withdraw the deposit"
  // string in this file unreachable, so the board offered no way to take one
  // back at all.
  const live =
    tx.status === "active" ||
    tx.status === "sharing" ||
    tx.status === "paused" ||
    tx.status === "stalled" ||
    tx.status === "deposited" ||
    tx.status === "cancelling";
  const done =
    tx.status === "completed" ||
    tx.status === "cancelled" ||
    tx.status === "failed";

  const items: MenuItem[] = [];
  if (tx.status === "sharing") {
    items.push({
      key: "share",
      label: t("transfers.shareDetails"),
      icon: <Icon.Info size={13} />,
      onSelect: () => openShare(tx.id),
    });
  }
  if (tx.status === "active") {
    items.push({
      key: "pause",
      label: t("transfers.pause"),
      icon: <Icon.Pause size={13} />,
      onSelect: () => fire(pause(tx.id)),
    });
  }
  if (tx.status === "paused" || tx.status === "stalled") {
    items.push({
      key: "resume",
      label: t("transfers.resume"),
      icon: <Icon.Play size={13} />,
      onSelect: () => fire(resume(tx.id)),
    });
  }
  if (tx.path) {
    // Opening the file comes first: after a download, the thing you want is the
    // thing, and the folder is how you go looking for it when you want something
    // else — to move it, to check what else is there.
    items.push({
      key: "openfile",
      label: t("transfers.openFile"),
      icon: <Icon.External size={13} />,
      onSelect: () => openFile(),
    });
    items.push({
      key: "open",
      label: t("transfers.openFolder"),
      icon: <Icon.Folder size={13} />,
      // `revealItemInDir` rather than opening the path: it shows the file
      // *selected* in the file manager, which is what someone who just received
      // something actually wants — opening its folder loses it among the rest.
      onSelect: () => {
        revealItemInDir(tx.path!).catch((e: unknown) =>
          toast.bad(t("transfers.openFolderFailed"), String(e))
        );
      },
    });
  }
  if (live) {
    items.push({
      key: "cancel",
      label:
        tx.status === "deposited"
          ? t("transfers.revokeDeposit")
          : t("transfers.cancel"),
      icon: <Icon.Stop size={13} />,
      danger: true,
      separated: true,
      onSelect: () => setConfirmCancel(true),
    });
  }
  if (done) {
    items.push({
      key: "remove",
      label: t("transfers.removeRow"),
      icon: <Icon.Trash size={13} />,
      danger: true,
      separated: true,
      onSelect: () => fire(removeRow(tx.key)),
    });
  }

  // A parked offer is a decision, not a status: the whole row opens the dialog.
  const isOffer = tx.status === "incoming";
  /** Open the received file with whatever the system uses for it. */
  const openFile = () => {
    if (!tx.path) return;
    openPath(tx.path).catch((e: unknown) =>
      toast.bad(t("transfers.openFileFailed"), String(e))
    );
  };
  // What clicking the row does, when it does anything: review an offer, or look at
  // what a share has served. Both are rows whose whole content is behind a panel;
  // a transfer in flight shows everything it has on the row itself, so opening it
  // would lead nowhere.
  const openable = isOffer
    ? () => openIncoming(tx.offerId!)
    : tx.status === "sharing"
      ? () => openShare(tx.id)
      : null;

  return (
    <>
      <div
        className={`row dir-${tx.dir} ${done ? "is-done" : ""} ${drag.className}`}
        style={
          openable ? { cursor: "pointer", ...drag.style } : drag.style
        }
        onClick={openable ?? undefined}
        onDoubleClick={tx.path ? openFile : undefined}
        role={openable ? "button" : undefined}
        tabIndex={openable ? 0 : undefined}
        onKeyDown={
          openable
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openable();
                }
              }
            : undefined
        }
      >
        {/* The file-kind chip doubles as the drag handle: it is the one part of
            a row that is always there, always the same size, and never a
            control on its own. Dragging it moves the row within its section;
            the arrow keys do the same thing one step at a time, which is the
            whole of the keyboard path now that the menu no longer has one. */}
        <span
          className="row-grip"
          {...grip}
          role="button"
          tabIndex={0}
          aria-label={t("transfers.reorder", tx.name)}
          onKeyDown={(e) => {
            if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
            e.preventDefault();
            e.stopPropagation();
            onNudge(e.key === "ArrowUp" ? -1 : 1);
          }}
        >
          <ExtChip name={tx.name} />
        </span>

        <div className="row-main">
          <div className="row-name truncate" title={tx.name}>
            {tx.name}
          </div>
          <div className="row-meta">
            <span className={`tone-${st.tone}`} style={{ fontWeight: 600 }}>
              {st.text}
            </span>
            {tx.handle && (
              <>
                <span className="sep" />
                {/* The same 8-hex handle `arvolo status` prints: with it on the
                    row, "arvolo cancel <this>" works from either surface. */}
                <span className="mono" style={{ fontSize: 10.5 }} title="arvolo cancel/pause/resume">
                  {tx.handle}
                </span>
              </>
            )}
            {tx.peer && (
              <>
                <span className="sep" />
                <span className="truncate" title={tx.peerId}>
                  {tx.dir === "out" ? t("common.to") : t("common.from")}{" "}
                  {tx.peer}
                </span>
              </>
            )}
            {tx.verified && (
              <Icon.Shield
                size={11}
                className="tone-ok"
                label={t("transfers.verifiedIdentity")}
              />
            )}
            {tx.size > 0 && (
              <>
                <span className="sep" />
                <span className="tnum">{fmtBytes(tx.size)}</span>
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
                <span className="tnum" title={t("transfers.swarm")}>
                  ⇄ {t("transfers.peers", peers)}
                </span>
              </>
            )}
          </div>
          {(live || tx.status === "completed") && <Progress t={tx} />}
          {tx.code && (
            <div className="hstack-sm" style={{ marginTop: 6 }}>
              <Icon.Qr size={12} className="t-mut" />
              <code className="mono t-xs">{tx.code}</code>
              <span className="t-xs t-mut">{t("transfers.liveCode")}</span>
            </div>
          )}
        </div>

        <div className="row-actions" onClick={(e) => e.stopPropagation()}>
          {isOffer ? (
            <Button
              size="sm"
              variant="in"
              onClick={() => openIncoming(tx.offerId!)}
            >
              {t("transfers.review")}
            </Button>
          ) : (
            <>
              {tx.status === "active" && (
                <IconButton
                  label={t("transfers.pause")}
                  onClick={() => fire(pause(tx.id))}
                >
                  <Icon.Pause size={14} />
                </IconButton>
              )}
              {(tx.status === "paused" || tx.status === "stalled") && (
                <IconButton
                  label={t("transfers.resume")}
                  onClick={() => fire(resume(tx.id))}
                >
                  <Icon.Play size={14} />
                </IconButton>
              )}
              <MenuButton
                items={items}
                label={t("transfers.rowActions", tx.name)}
              >
                <Icon.More size={15} />
              </MenuButton>
            </>
          )}
        </div>
      </div>

      <Confirm
        open={confirmCancel}
        title={
          tx.status === "deposited"
            ? t("transfers.confirmRevokeTitle")
            : t("transfers.confirmCancelTitle")
        }
        body={
          tx.status === "deposited"
            ? t(
                "transfers.confirmRevokeBody",
                tx.peer ?? t("transfers.confirmRevokePeer")
              )
            : t("transfers.confirmCancelBody", tx.name)
        }
        confirmLabel={
          tx.status === "deposited"
            ? t("transfers.confirmRevokeLabel")
            : t("transfers.confirmCancelLabel")
        }
        cancelLabel={t("transfers.keepGoing")}
        danger
        onCancel={() => setConfirmCancel(false)}
        onConfirm={() => {
          setConfirmCancel(false);
          fire(cancel(tx.id));
        }}
      />
    </>
  );
}

// One section's rows, and the ordering gesture that belongs to them.
//
// A section, not a column, is the list you can drag inside: the sections are
// what the board actually draws as contiguous runs of rows, and dropping a
// row into a place it cannot be drawn — "recently finished", above the ones
// still running — would leave the pointer and the result in different spots.
// `reorderItems` only permutes the ranks these rows already hold, so the rest
// of the column stays where it was.
function SectionRows({ items }: { items: UITransfer[] }) {
  const reorderItems = useStore((s) => s.reorderItems);
  const rows = useMemo(
    () => items.slice().sort((a, b) => b.rank - a.rank),
    [items]
  );
  const keys = useMemo(() => rows.map((r) => r.key), [rows]);
  const { listRef, gripProps, rowProps, active } = useReorder(
    keys,
    reorderItems
  );

  const nudge = useCallback(
    (key: string, delta: 1 | -1) => {
      const i = keys.indexOf(key);
      const j = i + delta;
      if (i < 0 || j < 0 || j >= keys.length) return;
      const order = keys.slice();
      [order[i], order[j]] = [order[j], order[i]];
      reorderItems(order);
    },
    [keys, reorderItems]
  );

  return (
    <div className={`card rows ${active ? "is-reordering" : ""}`} ref={listRef}>
      {rows.map((row, i) => (
        <Row
          key={row.key}
          t={row}
          grip={gripProps(row.key)}
          drag={rowProps(i)}
          onNudge={(delta) => nudge(row.key, delta)}
        />
      ))}
    </div>
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
  const t = useT();
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
            title={
              dir === "out"
                ? t("transfers.emptyOutTitle")
                : t("transfers.emptyInTitle")
            }
          >
            {dir === "out"
              ? t("transfers.emptyOutBody")
              : t("transfers.emptyInBody")}
            <div style={{ marginTop: 10 }}>
              <Button
                size="sm"
                onClick={() => (dir === "out" ? openSheet([]) : openReceive())}
              >
                {dir === "out"
                  ? t("transfers.emptyOutAction")
                  : t("transfers.emptyInAction")}
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
            <SectionRows items={sec.items} />
          </div>
        ))
      )}
    </section>
  );
}

export function TransfersView() {
  const t = useT();
  const transfers = useStore((s) => s.transfers);
  const search = useStore((s) => s.search);
  const openSheet = useStore((s) => s.openSheet);

  const rows = useMemo(() => Object.values(transfers), [transfers]);
  // `t` is in the dependency list because `sectionsFor` names its sections: the
  // memo has to be thrown away when the language changes, or the board would
  // keep the previous language's headings until a row moved.
  const out = useMemo(() => sectionsFor(rows, "out", search), [rows, search, t]);
  const inc = useMemo(() => sectionsFor(rows, "in", search), [rows, search, t]);

  // A first-run window with nothing in it should say what to do, not show two
  // empty columns. Past that the columns carry their own empty states, which are
  // quieter and keep the layout stable as rows come and go.
  if (!rows.length) {
    return (
      <div className="view-narrow" style={{ paddingTop: 30 }}>
        <button className="dropzone" onClick={() => openSheet([])}>
          <Icon.Send size={30} className="tone-out" />
          <div className="t-head">{t("transfers.firstRunTitle")}</div>
          <div className="t-sm t-mut" style={{ maxWidth: "44ch" }}>
            {t("transfers.firstRunBody")}
          </div>
          <span className="btn btn-primary" style={{ marginTop: 6 }}>
            <Icon.Send size={14} />
            {t("transfers.firstRunAction")}
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
      <Column dir="out" sections={out} title={t("transfers.outgoing")} />
      <Column dir="in" sections={inc} title={t("transfers.incoming")} />
    </div>
  );
}

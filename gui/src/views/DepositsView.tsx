// Everything you have left on a relay and can still take back.
//
// This list is the one place in the app that is *not* event-driven, and the
// distinction matters enough to be visible: nothing reports back when a link is
// downloaded or a sealed deposit collected, so the daemon has to go and ask the
// relay each time the panel is built. That means three states, not two — there,
// gone, and **could not ask** — and the third must never be rendered as either
// of the others. A one-shot link that has already been used, shown as "Attivo"
// because the relay was briefly unreachable, is exactly the failure this screen
// exists to prevent. `depositMeta` in format.ts owns that judgement.

import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { fire, useStore } from "../store";
import { depositMeta, fmtBytes } from "../format";
import { Icon } from "../ui/Icons";
import { Badge, Button, Empty, IconButton } from "../ui/Primitives";
import { CopyButton, ExtChip } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { DepositDto } from "../types";

function DepositRow({ d }: { d: DepositDto }) {
  const revoke = useStore((s) => s.revokeDeposit);
  const revoking = useStore((s) => s.revoking.includes(d.id));
  const peerLabel = useStore((s) => s.peerLabel);
  const [confirm, setConfirm] = useState(false);

  const m = depositMeta(d);
  const isLink = d.kind === "link";

  return (
    <>
      <div className="row" style={{ gridTemplateColumns: "38px 1fr auto" }}>
        <ExtChip name={d.name} />
        <div className="row-main">
          <div className="row-name truncate" title={d.name}>
            {d.name}
          </div>
          <div className="row-meta">
            <span className={`tone-${m.tone}`} style={{ fontWeight: 600 }}>
              {m.text}
            </span>
            <span className="sep" />
            <span className="tnum">{fmtBytes(d.size)}</span>
            <span className="sep" />
            <span className="truncate">
              {isLink ? m.detail : `sigillato per ${peerLabel(d.recipient || null)} · ${m.detail}`}
            </span>
          </div>
          {isLink && d.link && (
            <div className="copyfield" style={{ marginTop: 8 }}>
              <code className="mono">{d.link}</code>
              <IconButton
                label="Apri nel browser"
                onClick={() =>
                  openUrl(d.link).catch((e: unknown) =>
                    toast.bad("Non riesco ad aprire il link", String(e))
                  )
                }
              >
                <Icon.External size={14} />
              </IconButton>
              <CopyButton value={d.link} />
            </div>
          )}
        </div>
        <div className="hstack-sm">
          <Badge kind={isLink ? "dev" : "info"}>
            {isLink ? (
              <>
                <Icon.Link size={10} /> Link pubblico
              </>
            ) : (
              <>
                <Icon.Mailbox size={10} /> Deposito
              </>
            )}
          </Badge>
          <Button
            size="sm"
            variant="danger"
            busy={revoking}
            disabled={revoking}
            onClick={() => setConfirm(true)}
          >
            {m.revocable ? "Revoca" : "Rimuovi"}
          </Button>
        </div>
      </div>

      <Confirm
        open={confirm}
        title={m.revocable ? "Revocare?" : "Rimuovere il promemoria?"}
        body={
          m.revocable ? (
            isLink ? (
              <>
                Il link smette di funzionare <strong>per tutti</strong> quelli a
                cui l'hai dato, immediatamente e senza possibilità di ripensarci.
                Il file resta sul tuo disco.
              </>
            ) : (
              <>
                Il file viene tolto dal relay e l'offerta ritirata dalla casella
                del destinatario. Se non l'ha ancora ritirato, non potrà più farlo.
              </>
            )
          ) : (
            "Sul relay non c'è più niente da togliere: sparisce solo questa riga."
          )
        }
        confirmLabel={m.revocable ? "Revoca" : "Rimuovi"}
        danger
        busy={revoking}
        onCancel={() => setConfirm(false)}
        onConfirm={async () => {
          setConfirm(false);
          await revoke(d.id);
        }}
      />
    </>
  );
}

export function DepositsView() {
  const deposits = useStore((s) => s.deposits);
  const loading = useStore((s) => s.depositsLoading);
  const error = useStore((s) => s.depositsError);
  const reload = useStore((s) => s.loadDeposits);
  const openSheet = useStore((s) => s.openSheet);

  const links = deposits.filter((d) => d.kind === "link");
  const sealed = deposits.filter((d) => d.kind !== "link");

  return (
    <div className="stack">
      <div className="hstack">
        <div className="grow t-sm t-sec">
          Quello che hai lasciato su un relay e puoi ancora ritirare. Lo stato
          viene chiesto al relay ogni volta che apri questa schermata — non c'è
          modo di saperlo altrimenti.
        </div>
        <Button size="sm" onClick={() => fire(reload())} busy={loading}>
          <Icon.Refresh size={13} /> Aggiorna
        </Button>
      </div>

      {error && (
        <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
          {error}
        </div>
      )}

      {deposits.length === 0 && !loading ? (
        <div className="card">
          <Empty
            icon={<Icon.Link size={22} />}
            title="Nessun link o deposito attivo"
            action={
              <Button variant="primary" onClick={() => openSheet([])}>
                <Icon.Send size={14} /> Crea un link
              </Button>
            }
          >
            Quando crei un link pubblico o depositi un file nella casella di
            qualcuno, compare qui — e da qui puoi revocarlo.
          </Empty>
        </div>
      ) : (
        <>
          {links.length > 0 && (
            <div className="section">
              <div className="section-head">
                <span className="t-label">Link pubblici</span>
                <span className="t-xs t-mut tnum">{links.length}</span>
              </div>
              <div className="card rows">
                {links.map((d) => (
                  <DepositRow key={d.id} d={d} />
                ))}
              </div>
            </div>
          )}
          {sealed.length > 0 && (
            <div className="section">
              <div className="section-head">
                <span className="t-label">Depositi sigillati</span>
                <span className="t-xs t-mut tnum">{sealed.length}</span>
              </div>
              <div className="card rows">
                {sealed.map((d) => (
                  <DepositRow key={d.id} d={d} />
                ))}
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

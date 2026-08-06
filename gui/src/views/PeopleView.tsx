// The address book — and, because this app's promise rests on it, the place
// where trust is granted and taken back.
//
// Three marks, three different things, deliberately never collapsed into one
// "trusted" toggle:
//
//   verified — you compared the fingerprint out of band. Evidence.
//   trusted  — their files download without asking you. A convenience that is
//              only safe *because* of the first, which is why the daemon refuses
//              to grant it to an unverified key unless it is forced, and why the
//              forcing dialog spells out what is being waived.
//   blocked  — their offers are dropped on arrival, silently.
//
// Import/export live here rather than behind a menu because moving an address
// book between machines is a real thing people do, and because the import path
// has one rule worth surfacing: it never rebinds a name you already use, since
// that would be a key change nobody asked about.

import { useMemo, useRef, useState } from "react";
import { save as saveDialog, open as openDialog } from "@tauri-apps/plugin-dialog";
import { fire, useStore } from "../store";
import { api } from "../ipc";
import { Icon } from "../ui/Icons";
import {
  Badge,
  Button,
  Empty,
  Field,
  Segmented,
  TextInput,
  TrustBadges,
} from "../ui/Primitives";
import { Avatar, CopyField, Fingerprint } from "../ui/Bits";
import { MenuButton, type MenuItem } from "../ui/Menu";
import { Confirm, Sheet } from "../ui/Sheet";
import { toast } from "../ui/Toasts";
import type { ContactDto } from "../types";

type Filter = "all" | "verified" | "trusted" | "blocked";

// ---------------------------------------------------------------------------

function PersonCard({ c }: { c: ContactDto }) {
  const openSheet = useStore((s) => s.openSheet);
  const openPerson = useStore((s) => s.openPerson);
  const markUnverified = useStore((s) => s.markUnverified);
  const markTrusted = useStore((s) => s.markTrusted);
  const markUntrusted = useStore((s) => s.markUntrusted);
  const blockContact = useStore((s) => s.blockContact);
  const unblockContact = useStore((s) => s.unblockContact);
  const acceptName = useStore((s) => s.acceptName);
  const removeContact = useStore((s) => s.removeContact);

  const [confirmRemove, setConfirmRemove] = useState(false);
  const [confirmForce, setConfirmForce] = useState(false);

  const items: MenuItem[] = [
    {
      key: "detail",
      label: "Dettagli e impronta",
      icon: <Icon.Info size={13} />,
      onSelect: () => openPerson(c.name),
    },
    c.verified
      ? {
          key: "unverify",
          label: "Togli la verifica",
          icon: <Icon.Shield size={13} />,
          onSelect: () => fire(markUnverified(c.name)),
        }
      : {
          key: "verify",
          label: "Segna come verificato…",
          icon: <Icon.Shield size={13} />,
          onSelect: () => openPerson(c.name),
        },
    c.trusted
      ? {
          key: "untrust",
          label: "Non scaricare più in automatico",
          icon: <Icon.Star size={13} />,
          onSelect: () => fire(markUntrusted(c.name)),
        }
      : {
          key: "trust",
          label: "Scarica in automatico",
          icon: <Icon.Star size={13} />,
          onSelect: () => {
            if (c.verified) fire(markTrusted(c.name, false));
            else setConfirmForce(true);
          },
        },
    c.blocked
      ? {
          key: "unblock",
          label: "Sblocca",
          icon: <Icon.Ban size={13} />,
          separated: true,
          onSelect: () => fire(unblockContact(c.name)),
        }
      : {
          key: "block",
          label: "Blocca",
          icon: <Icon.Ban size={13} />,
          danger: true,
          separated: true,
          onSelect: () => fire(blockContact(c.name)),
        },
    {
      key: "remove",
      label: "Rimuovi dalla rubrica",
      icon: <Icon.Trash size={13} />,
      danger: true,
      onSelect: () => setConfirmRemove(true),
    },
  ];

  return (
    <>
      <div className={`person ${c.blocked ? "blocked" : ""}`}>
        <div className="person-top">
          <Avatar name={c.display_name || c.name} id={c.id} size={38} />
          <div className="grow" style={{ minWidth: 0 }}>
            <div className="truncate" style={{ fontWeight: 620 }}>
              {c.name}
            </div>
            <div className="t-xs t-mut truncate">
              {c.display_name && c.display_name !== c.name
                ? `si presenta come “${c.display_name}”`
                : " "}
            </div>
          </div>
          <MenuButton items={items} label={`Azioni per ${c.name}`}>
            <Icon.More size={15} />
          </MenuButton>
        </div>

        <div className="hstack-sm wrap">
          <TrustBadges
            verified={c.verified}
            trusted={c.trusted}
            blocked={c.blocked}
          />
          {!c.verified && !c.blocked && (
            <Badge kind="warn" title="L'impronta non è mai stata confrontata">
              Non verificato
            </Badge>
          )}
        </div>

        {c.pending_name && (
          <div
            className="card card-pad t-xs"
            style={{ borderColor: "var(--amber)", padding: 10 }}
          >
            Vuole farsi chiamare “{c.pending_name}”.
            <div className="hstack-sm" style={{ marginTop: 7 }}>
              <Button size="sm" onClick={() => fire(acceptName(c.name))}>
                Approva
              </Button>
            </div>
          </div>
        )}

        <button
          className="fingerprint truncate"
          title={`${c.fingerprint}\n${c.id}`}
          onClick={() => openPerson(c.name)}
          style={{
            border: 0,
            background: "transparent",
            padding: 0,
            textAlign: "left",
            cursor: "pointer",
          }}
        >
          {c.fingerprint}
        </button>

        <div className="person-acts">
          <Button
            size="sm"
            variant="primary"
            disabled={c.blocked}
            onClick={() => openSheet([], c.name)}
          >
            <Icon.Send size={13} /> Invia
          </Button>
          <Button size="sm" onClick={() => openPerson(c.name)}>
            Dettagli
          </Button>
        </div>
      </div>

      <Confirm
        open={confirmRemove}
        title={`Rimuovere ${c.name}?`}
        body="Sparisce dalla rubrica insieme ai suoi contrassegni di verifica e fiducia. I trasferimenti già fatti restano nella cronologia."
        confirmLabel="Rimuovi"
        danger
        onCancel={() => setConfirmRemove(false)}
        onConfirm={() => {
          setConfirmRemove(false);
          fire(removeContact(c.name));
        }}
      />

      <Confirm
        open={confirmForce}
        title="Scaricare in automatico da una chiave non verificata?"
        body={
          <>
            I file di <strong>{c.name}</strong> verrebbero scaricati senza
            chiederti niente, ma non hai mai confrontato la sua impronta di
            persona. Se qualcuno si fosse messo in mezzo quando l'hai aggiunto,
            staresti scaricando in automatico da lui.
            <div style={{ marginTop: 10 }}>
              <Fingerprint value={c.fingerprint} />
            </div>
            <div style={{ marginTop: 10 }}>
              La strada giusta è confrontare l'impronta e poi segnarlo come
              verificato.
            </div>
          </>
        }
        confirmLabel="Forza comunque"
        cancelLabel="Verifico prima"
        danger
        onCancel={() => setConfirmForce(false)}
        onConfirm={() => {
          setConfirmForce(false);
          fire(markTrusted(c.name, true));
        }}
      />
    </>
  );
}

// ---------------------------------------------------------------------------

function AddPersonSheet({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const addContact = useStore((s) => s.addContact);
  const [name, setName] = useState("");
  const [id, setId] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    try {
      await addContact(name.trim(), id.trim());
      toast.ok(
        `Salvato ${name.trim()}`,
        "Resta non verificato finché non confronti l'impronta."
      );
      setName("");
      setId("");
      onClose();
    } catch {
      // reported by the store
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      open={open}
      onClose={onClose}
      placement="center"
      title="Aggiungi per id"
      subtitle="La strada lunga: serve il suo id pubblico per intero."
      footer={
        <>
          <div className="spacer" />
          <Button onClick={onClose} disabled={busy}>
            Annulla
          </Button>
          <Button
            variant="primary"
            busy={busy}
            disabled={!name.trim() || !id.trim() || busy}
            onClick={submit}
          >
            Salva
          </Button>
        </>
      }
    >
      <Field label="Come lo chiami">
        {({ id: fid }) => (
          <TextInput
            id={fid}
            data-autofocus
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
            placeholder="es. Giulia"
          />
        )}
      </Field>
      <Field
        label="Id pubblico"
        hint="Glielo dà «arvolo me», oppure la schermata Impostazioni della sua app."
      >
        {({ id: fid, describedBy }) => (
          <TextInput
            id={fid}
            aria-describedby={describedBy}
            className="mono"
            spellCheck={false}
            autoCapitalize="off"
            value={id}
            onChange={(e) => setId(e.currentTarget.value)}
            placeholder="if2xmne…"
          />
        )}
      </Field>
      <div className="card card-pad t-sm t-sec">
        Molto più semplice: <strong>Scambia contatti</strong>. Vi leggete un
        codice corto e vi ritrovate entrambi salvati e già verificati, senza
        copiare cinquanta caratteri.
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------------------

/** The detail sheet: everything about one person, and the only place the
 *  verification ritual can be completed. It is a ritual on purpose — reading the
 *  fingerprint and confirming are two separate acts, so marking someone verified
 *  can never be a side effect of glancing at their card. */
function PersonSheet() {
  const name = useStore((s) => s.personOpen);
  const close = useStore((s) => s.openPerson);
  const contacts = useStore((s) => s.contacts);
  const markVerified = useStore((s) => s.markVerified);
  const markUnverified = useStore((s) => s.markUnverified);
  const renameContact = useStore((s) => s.renameContact);

  const c = contacts.find((x) => x.name === name);
  const [checked, setChecked] = useState(false);
  const [newName, setNewName] = useState("");

  if (!c) return null;

  return (
    <Sheet
      open
      onClose={() => {
        setChecked(false);
        setNewName("");
        close(null);
      }}
      title={c.name}
      subtitle={c.display_name ? `si presenta come “${c.display_name}”` : undefined}
    >
      <div className="hstack">
        <Avatar name={c.display_name || c.name} id={c.id} size={48} />
        <div className="hstack-sm wrap grow">
          <TrustBadges
            verified={c.verified}
            trusted={c.trusted}
            blocked={c.blocked}
          />
        </div>
      </div>

      <Field
        label="Impronta"
        hint="Le stesse parole devono comparire sulla sua schermata. Confrontatele a voce o di persona — non via chat sullo stesso canale da cui vi siete scambiati l'id."
      >
        {() => (
          <div
            className="card card-pad"
            style={{ background: "var(--surface-2)" }}
          >
            <Fingerprint value={c.fingerprint} />
          </div>
        )}
      </Field>

      <Field label="Id pubblico">
        {() => <CopyField value={c.id} wrap />}
      </Field>

      {c.verified ? (
        <div className="card card-pad stack-sm">
          <div className="hstack-sm">
            <Icon.Shield className="tone-ok" />
            <strong>Verificato</strong>
          </div>
          <div className="t-sm t-sec">
            Hai confermato questa impronta fuori banda.
          </div>
          <div>
            <Button size="sm" onClick={() => fire(markUnverified(c.name))}>
              Togli la verifica
            </Button>
          </div>
        </div>
      ) : (
        <div
          className="card card-pad stack-sm"
          style={{ borderColor: "var(--amber)" }}
        >
          <strong>Non ancora verificato</strong>
          <div className="t-sm t-sec">
            Finché non confronti l'impronta, l'unica cosa che sai è che qualcuno
            ti ha dato quell'id.
          </div>
          <label className="hstack" style={{ cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={checked}
              onChange={(e) => setChecked(e.currentTarget.checked)}
            />
            <span className="t-sm">
              Ho confrontato l'impronta con {c.name} fuori da questa app.
            </span>
          </label>
          <div>
            <Button
              size="sm"
              variant="primary"
              disabled={!checked}
              onClick={() => {
                setChecked(false);
                fire(markVerified(c.name));
              }}
            >
              <Icon.Shield size={13} /> Segna come verificato
            </Button>
          </div>
        </div>
      )}

      <Field label="Rinomina" hint="Il nome è tuo: la chiave e i contrassegni restano.">
        {({ id }) => (
          <div className="hstack-sm">
            <TextInput
              id={id}
              value={newName}
              onChange={(e) => setNewName(e.currentTarget.value)}
              placeholder={c.name}
            />
            <Button
              size="sm"
              disabled={!newName.trim() || newName.trim() === c.name}
              onClick={async () => {
                const to = newName.trim();
                await renameContact(c.name, to);
                setNewName("");
                close(to);
              }}
            >
              Rinomina
            </Button>
          </div>
        )}
      </Field>
    </Sheet>
  );
}

// ---------------------------------------------------------------------------

export function PeopleView() {
  const contacts = useStore((s) => s.contacts);
  const startPairing = useStore((s) => s.startPairing);
  const addContact = useStore((s) => s.addContact);
  const pruneNames = useStore((s) => s.pruneNames);

  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  const [addOpen, setAddOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const busyRef = useRef(false);

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return contacts.filter((c) => {
      if (filter === "verified" && !c.verified) return false;
      if (filter === "trusted" && !c.trusted) return false;
      if (filter === "blocked" && !c.blocked) return false;
      if (filter !== "blocked" && c.blocked) return false;
      if (!needle) return true;
      return (
        c.name.toLowerCase().includes(needle) ||
        c.display_name.toLowerCase().includes(needle) ||
        c.id.toLowerCase().startsWith(needle)
      );
    });
  }, [contacts, filter, q]);

  const blockedCount = contacts.filter((c) => c.blocked).length;

  /** Export mirrors `arvolo contacts export` byte for byte: name, id and the two
   *  marks, and nothing that describes *this* machine. */
  const doExport = async () => {
    const rows = contacts.map((c) => ({
      name: c.name,
      id: c.id,
      verified: c.verified,
      trusted: c.trusted,
    }));
    const path = await saveDialog({
      defaultPath: "arvolo-contatti.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    try {
      await api.writeTextFile(path, JSON.stringify(rows, null, 2));
      toast.ok(
        `Esportati ${rows.length} contatti`,
        "Il file contiene solo id pubblici: nessun segreto."
      );
    } catch (e) {
      toast.bad("Esportazione non riuscita", String(e));
    }
  };

  const doImport = async () => {
    if (busyRef.current) return;
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof path !== "string") return;
    busyRef.current = true;
    setImporting(true);
    try {
      const text = await api.readTextFile(path);
      const rows = JSON.parse(text);
      if (!Array.isArray(rows)) throw new Error("il file non è una lista");

      const existing = new Set(contacts.map((c) => c.name));
      let added = 0;
      let skipped = 0;
      for (const row of rows) {
        const name = typeof row?.name === "string" ? row.name : null;
        const id = typeof row?.id === "string" ? row.id : null;
        if (!name || !id) continue;
        // Never rebind a name already in the book: an import that overwrote an
        // existing contact's key would be a key change nobody asked about.
        if (existing.has(name)) {
          skipped++;
          continue;
        }
        try {
          await addContact(name, id);
          added++;
          existing.add(name);
        } catch {
          skipped++;
        }
      }
      toast.ok(
        `Importati ${added} contatti`,
        `${skipped ? `${skipped} saltati. ` : ""}Tutti non verificati: i contrassegni non si importano, perché quelle impronte non le hai controllate tu.`
      );
    } catch (e) {
      toast.bad("Importazione non riuscita", String(e));
    } finally {
      busyRef.current = false;
      setImporting(false);
    }
  };

  return (
    <div className="stack">
      <div className="hstack wrap">
        <Button variant="primary" onClick={() => fire(startPairing("contact_host"))}>
          <Icon.Qr size={14} /> Scambia contatti
        </Button>
        <Button onClick={() => fire(startPairing("contact_join"))}>
          Ho un codice
        </Button>
        <Button onClick={() => setAddOpen(true)}>
          <Icon.Plus size={13} /> Per id
        </Button>
        <div className="spacer grow" />
        <Button size="sm" onClick={doExport} disabled={!contacts.length}>
          Esporta
        </Button>
        <Button size="sm" onClick={doImport} busy={importing}>
          Importa
        </Button>
        <MenuButton
          label="Altre azioni sulla rubrica"
          items={[
            {
              key: "prune",
              label: "Ripulisci i nomi orfani",
              icon: <Icon.Trash size={13} />,
              onSelect: async () => {
                const n = await pruneNames();
                toast.ok(
                  n
                    ? `Rimossi ${n} record`
                    : "Niente da ripulire",
                  "Sono nomi annunciati da contatti che non hai più."
                );
              },
            },
          ]}
        >
          <Icon.More size={15} />
        </MenuButton>
      </div>

      <div className="hstack wrap">
        <Segmented
          label="Filtro rubrica"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "Tutti" },
            { value: "verified", label: "Verificati" },
            { value: "trusted", label: "Fidati" },
            {
              value: "blocked",
              label: blockedCount ? `Bloccati (${blockedCount})` : "Bloccati",
            },
          ]}
        />
        <div className="grow" style={{ maxWidth: 280 }}>
          <TextInput
            value={q}
            onChange={(e) => setQ(e.currentTarget.value)}
            placeholder="Cerca per nome o id…"
            aria-label="Cerca in rubrica"
          />
        </div>
      </div>

      {shown.length === 0 ? (
        <div className="card">
          <Empty
            icon={<Icon.People size={22} />}
            title={
              contacts.length === 0
                ? "Nessuno in rubrica"
                : "Nessun contatto corrisponde"
            }
            action={
              contacts.length === 0 ? (
                <Button
                  variant="primary"
                  onClick={() => fire(startPairing("contact_host"))}
                >
                  <Icon.Qr size={14} /> Scambia contatti
                </Button>
              ) : undefined
            }
          >
            {contacts.length === 0
              ? "Il modo più rapido per aggiungere qualcuno è leggergli un codice corto: vi salvate a vicenda e siete già verificati, senza copiare id a mano."
              : "Prova a cambiare filtro o ricerca."}
          </Empty>
        </div>
      ) : (
        <div className="people-grid">
          {shown.map((c) => (
            <PersonCard key={c.name} c={c} />
          ))}
        </div>
      )}

      <AddPersonSheet open={addOpen} onClose={() => setAddOpen(false)} />
      <PersonSheet />
    </div>
  );
}

// Scambia: il posto da cui tutto parte.
//
// Sezione 15a del design di riferimento ("Arvolo Mobile 2 — Scambia"), che
// sostituisce la lista di risposte dell'11a: la camera è una scheda blu notte in
// testa («Ce l'ho davanti»); sotto, un campo che cerca una persona o un
// dispositivo **e riconosce un codice ricevuto** (13e: incollato o scritto, il
// riquadro dice cosa ha letto e offre Ricevi); poi quello che un tuo
// dispositivo ti ha lasciato (Prendi); i tuoi dispositivi, quadrati, con il
// link pubblico tratteggiato in coda; le persone, tonde, con l'anello blu se
// sono collegate e lo stato sotto il nome.
//
// Si sceglie una volta, e la seconda domanda (`ComposeView`) sa già a chi. Con
// un tocco lungo su un volto i volti diventano caselle da spuntare (16a) e la
// barra blu in fondo dice quante persone e come arriveranno.
//
// Se si arriva da *Condividi* i file ci sono già (11e): al posto del titolo c'è
// quello che hai in mano.

import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { fire, SELF_RECIPIENT, useStore } from "../store";
import { useT } from "../i18n";
import { fmtAgo, fmtBytes, isMedia } from "../format";
import { Icon } from "../ui/Icons";
import { haptic } from "../haptics";
import { shapeOf } from "../overlays/ReceiveSheet";
import type { ContactDto, DeviceDto } from "../types";

/** How recently a machine has to have run a sync round to be drawn as awake.
 *
 *  Twice the round, and deliberately soft: `last_seen` is **not** a presence
 *  light (see `DeviceDto`) — it moves once every five minutes, by that machine's
 *  own clock. So the dot means "was here a moment ago". Per-device presence
 *  would need a beacon that carries a device id; the relay's beacon is per
 *  *identity*, and all of these share one. */
const AWAKE_SECS = 11 * 60;
/** Quante facce stanno nelle due righe della griglia prima della Rubrica. */
const FACES = 8;
/** Quanto si tiene premuto un volto perché i volti diventino caselle (16a). */
const HOLD_MS = 450;
/** Ogni quanto si richiede al relay chi è collegato, finché la schermata è in vista. */
const PRESENCE_EVERY_MS = 30_000;

/** The glyph for a machine, by what kind it says it is. For an icon, never for a
 *  decision — `platform` is self-reported. */
export function deviceGlyph(platform: string, size = 24) {
  const p = platform.toLowerCase();
  if (p.includes("ipad") || p.includes("tablet")) return <Icon.Tablet size={size} />;
  if (p.includes("ios") || p.includes("android")) return <Icon.Phone size={size} />;
  if (p.includes("mac") || p.includes("darwin")) return <Icon.Laptop size={size} />;
  if (p.includes("win") || p.includes("linux")) return <Icon.Desktop size={size} />;
  return <Icon.Devices size={size} />;
}

/** The viewfinder's four corners. Nothing else: a frame with a border all the way
 *  round reads as a window the camera is behind, and the camera is the screen. */
export function Frame({ size }: { size: number }) {
  return (
    <span
      className={`exch-frame${size <= 140 ? " sm" : ""}`}
      style={{ width: size, height: size }}
      aria-hidden
    >
      <i className="tl" />
      <i className="tr" />
      <i className="bl" />
      <i className="br" />
    </span>
  );
}

export function isAwake(d: DeviceDto, nowSecs = Math.floor(Date.now() / 1000)) {
  return d.last_seen > 0 && nowSecs - d.last_seen < AWAKE_SECS;
}

/** The one letter on a face: the canvas draws initials as a single capital. */
export function initial(name: string): string {
  const c = name.trim().charAt(0);
  return c ? c.toUpperCase() : "?";
}

/** Un volto: l'iniziale su un disco, l'anello blu e il pallino se è collegata;
 *  in modalità scelta, la spunta. */
export function Face({
  name,
  online,
  picked,
  size = 64,
}: {
  name: string;
  online?: boolean;
  /** `undefined` when nobody is choosing; a boolean while the faces are boxes. */
  picked?: boolean;
  size?: number;
}) {
  const ring = online && picked === undefined;
  const r = size / 2 - 1.25;
  return (
    <span
      className={`face-pic${ring ? " ring" : ""}${picked ? " sel" : ""}`}
      style={{ width: size, height: size }}
      aria-hidden
    >
      {ring ? (
        <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} fill="none">
          <circle cx={size / 2} cy={size / 2} r={r} stroke="var(--in-fill)" strokeWidth="2.5" />
        </svg>
      ) : null}
      <span className="face-in">{initial(name)}</span>
      {online && picked === undefined ? <span className="face-dot" /> : null}
      {picked !== undefined ? (
        <span className="face-check">{picked ? <Icon.Check size={13} /> : null}</span>
      ) : null}
    </span>
  );
}

/** Lo stato sotto il nome: collegata adesso, quando vi siete scritti, o che non
 *  è salvata. */
function faceSub(c: ContactDto, online: boolean, t: ReturnType<typeof useT>): string {
  if (online) return t("exchange.online");
  if (c.last_exchange) return fmtAgo(c.last_exchange);
  return c.verified ? t("exchange.verified") : "";
}

/** A code or a ticket, as the field recognises it — anything else is a search. */
type Read = "code" | "chunk" | "mailbox";
function readOf(q: string): Read | null {
  const shape = shapeOf(q);
  return shape === "code" || shape === "chunk" || shape === "mailbox" ? shape : null;
}

export function ExchangeView() {
  const t = useT();
  const push = useStore((s) => s.push);
  const go = useStore((s) => s.go);
  const startCompose = useStore((s) => s.startCompose);
  const clearCompose = useStore((s) => s.clearCompose);
  const startPairing = useStore((s) => s.startPairing);
  const devices = useStore((s) => s.devices);
  const contacts = useStore((s) => s.contacts);
  const presence = useStore((s) => s.presence);
  const transfers = useStore((s) => s.transfers);
  const status = useStore((s) => s.status);
  const files = useStore((s) => s.composeFiles);
  const picking = useStore((s) => s.picking);
  const setPicking = useStore((s) => s.setPicking);
  const accept = useStore((s) => s.accept);
  const receive = useStore((s) => s.receive);
  const loadPresence = useStore((s) => s.loadPresence);
  const loadDevices = useStore((s) => s.loadDevices);

  const [query, setQuery] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  // Who is reachable right now is a question only the relay can answer, and
  // this screen is where the answer is drawn as a ring round a face. Keyed on
  // the id list rather than on mounting, as *Persone* does: on a fresh launch
  // the address book lands after this screen, and a probe at mount asks about
  // nobody. Then every half minute while the screen is in view. Your own
  // machines come from the register, which nothing pushes either.
  const ids = contacts.map((c) => c.id).join(",");
  useEffect(() => {
    fire(loadDevices());
  }, [loadDevices]);
  useEffect(() => {
    if (!ids) return;
    const probe = () => {
      if (!document.hidden) fire(loadPresence());
    };
    probe();
    const timer = window.setInterval(probe, PRESENCE_EVERY_MS);
    document.addEventListener("visibilitychange", probe);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", probe);
    };
  }, [ids, loadPresence]);

  // Sempre la schermata dei codici, anche quando non ce n'è nessuno: è lei che
  // sa dirlo e che porta a farne uno. Prima, senza codici attivi, apriva il
  // **pannello della finestra** in modalità «codice» — un posto diverso a
  // seconda di quanti codici avevi, e quello dei due che su un telefono non è
  // più la schermata d'invio.
  const showCode = () => push("codes");

  const others = devices.filter((d) => !d.is_this_device);
  const q = query.trim().toLowerCase();
  const read = readOf(query);
  const matches = (name: string) => !q || read !== null || name.toLowerCase().includes(q);

  // Quello che un tuo dispositivo ti ha lasciato: le offerte parcheggiate che
  // vengono dalla tua stessa identità. Il daemon le prende da solo quando
  // «Accetta automaticamente» glielo lascia fare; queste sono quelle rimaste.
  const myId = status?.public_id;
  const mine = Object.values(transfers).filter(
    (x) => x.status === "incoming" && !!myId && x.peerId === myId
  );
  const mineBytes = mine.reduce((n, x) => n + x.size, 0);
  const mineSince = mine.length ? Math.min(...mine.map((x) => x.firstSeen)) : 0;
  // Un'offerta non dice da quale delle tue macchine viene: se ne hai una sola
  // altra, è lei; altrimenti è «un tuo dispositivo».
  const mineFrom = others.length === 1 ? others[0].name : null;
  const takeAll = () => {
    haptic.did();
    for (const x of mine) if (x.offerId) fire(accept(x.offerId, null));
  };

  const people = contacts
    .filter((c) => !c.blocked && c.name !== SELF_RECIPIENT && matches(c.display_name || c.name))
    .sort((a, b) => (b.last_exchange ?? 0) - (a.last_exchange ?? 0));
  const faces = picking ? people : people.slice(0, FACES);

  const togglePick = (name: string) => {
    if (!picking) return;
    haptic.picked();
    setPicking(picking.includes(name) ? picking.filter((n) => n !== name) : [...picking, name]);
  };
  const pickedOnline = (picking ?? []).filter((n) => {
    const c = contacts.find((x) => x.name === n);
    return c && presence[c.id] === true;
  }).length;

  const receiveCode = async () => {
    if (!read || busy) return;
    setBusy(true);
    try {
      await receive(query.trim(), null, password || null);
      setQuery("");
      setPassword("");
      go("transfers");
    } catch {
      // `act` in the store already recorded the reason and raised the toast.
    } finally {
      setBusy(false);
    }
  };

  const bytes = files.reduce((n, f) => n + (f.size ?? 0), 0);

  return (
    <div className="ios-page exch-page">
      {files.length > 0 ? (
        /* 11e: i file prima del titolo. Il titolo lo dice la domanda. */
        <div className="files-head">
          {/* La via per lasciar perdere quello che si aveva in mano. */}
          <button className="icon-btn" aria-label={t("common.back")} onClick={clearCompose}>
            <Icon.ChevronLeft size={20} />
          </button>
          <span className="files-thumbs" aria-hidden>
            <span className="thumb" />
            {files.length > 1 ? <span className="thumb" /> : null}
          </span>
          <span className="grow" style={{ minWidth: 0 }}>
            <span className="files-line tnum">
              {files.every((f) => isMedia(f.name))
                ? t("compose.countPhotos", files.length)
                : t("compose.countFiles", files.length)}{" "}
              · {fmtBytes(bytes)}
            </span>
            <span className="files-ask">{t("send.toWhom")}</span>
          </span>
        </div>
      ) : (
        <div className="root-head">
          <h2 className="large-title">{t("exchange.title")}</h2>
          <div className="root-tools">
            <button className="act-tool" aria-label={t("exchange.showCode")} onClick={showCode}>
              <Icon.Qr size={19} />
            </button>
            <button className="act-tool" aria-label={t("title.settings")} onClick={() => push("settings")}>
              <Icon.Settings size={19} />
            </button>
          </div>
        </div>
      )}

      {/* Le due risposte che non hanno un nome, affiancate (18a).
          La camera — chi hai davanti inquadra o si fa inquadrare — e il link,
          per chi Arvolo non ce l'ha. Il link ha girato tre posti prima di
          trovare questo: dentro la griglia dei dispositivi (sotto un titolo che
          mentiva), in una sezione sua (una griglia da quattro colonne con una
          tile sola), e come riga in mezzo alle liste (un verbo travestito da
          voce d'elenco). Qui è quello che è: **un'alternativa alla camera**, e
          le due stanno una accanto all'altra perché è lì che si sceglie fra
          loro. La camera resta blu notte ed è più larga: è quella che si usa
          quasi sempre. */}
      <div className="cam-pair">
        <button className="cam-card" onClick={() => push("camera")}>
          <Frame size={44} />
          <span className="grow" style={{ minWidth: 0 }}>
            <span className="cam-card-title">{t("exchange.inFront")}</span>
            <span className="cam-card-sub">{t("exchange.aimTheirSquare")}</span>
          </span>
        </button>
        <button className="link-card" onClick={() => startCompose({ kind: "link" })}>
          <span className="link-card-ico" aria-hidden>
            <Icon.Globe size={22} />
          </span>
          <span className="grow" style={{ minWidth: 0 }}>
            <span className="cam-card-title">{t("exchange.linkCard")}</span>
            <span className="cam-card-sub">{t("exchange.linkCardSub")}</span>
          </span>
        </button>
      </div>

      {/* Cerca, e legge i codici. */}
      <label className={`xs-search${read ? " code" : ""}`}>
        <Icon.Search size={18} />
        <input
          className={read ? "mono" : undefined}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("exchange.searchPlaceholder")}
          aria-label={t("exchange.searchPlaceholder")}
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          enterKeyHint={read ? "go" : "search"}
          onKeyDown={(e) => {
            if (e.key === "Enter" && read) void receiveCode();
          }}
        />
        {query ? (
          <button className="xs-clear" aria-label={t("common.clear")} onClick={() => setQuery("")}>
            <Icon.Close size={11} />
          </button>
        ) : null}
      </label>

      {read ? (
        /* 13e: il riquadro dice cosa ha riconosciuto e offre Ricevi. */
        <>
          <div className="code-card" role="group" aria-label={t(`code.title.${read}` as "code.title.code")}>
            <div className="code-card-head">
              <span className="code-icon">
                {read === "code" ? <Icon.Qr size={22} /> : <Icon.Download size={22} />}
              </span>
              <div className="grow">
                <div className="code-title">{t(`code.title.${read}` as "code.title.code")}</div>
                <div className="code-sub">{t(`code.sub.${read}` as "code.sub.code")}</div>
              </div>
            </div>
            {read === "mailbox" ? (
              <input
                className="input"
                type="password"
                autoComplete="off"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder={t("receive.passwordLabel")}
                aria-label={t("receive.passwordLabel")}
              />
            ) : null}
            <button className="code-btn" disabled={busy} onClick={() => void receiveCode()}>
              <Icon.Download size={18} /> {t("code.receive")}
            </button>
          </div>
          {read === "code" ? <div className="code-note">{t("code.note")}</div> : null}
        </>
      ) : null}

      {mine.length > 0 && !read ? (
        <div className="take-row">
          <Icon.ArrowDownLeft size={19} />
          <div className="grow" style={{ minWidth: 0 }}>
            <div className="take-title tnum">{t("exchange.takeTitle", mine.length, mineFrom)}</div>
            <div className="take-sub tnum">
              {t("exchange.takeSub", fmtBytes(mineBytes), clockOf(mineSince))}
            </div>
          </div>
          <button className="take-btn" onClick={takeAll}>
            {t("exchange.take")}
          </button>
        </div>
      ) : null}

      <div className="xs-section">
        <div className="xs-head">
          <span className="xs-head-title">{t("exchange.yourDevices")}</span>
        </div>
        <div className="xs-grid">
          {others.filter((d) => matches(d.name)).map((d) => {
            const awake = isAwake(d);
            return (
              <button
                key={d.id}
                className={`xs-cell${awake ? "" : " cold"}`}
                onClick={() => startCompose({ kind: "self", device: d.name })}
              >
                <span className="dev-tile">
                  {deviceGlyph(d.platform, 30)}
                  {awake ? <span className="dev-dot" aria-hidden /> : null}
                  {mine.length > 0 && mineFrom === d.name ? (
                    <span className="dev-badge tnum" aria-hidden>
                      {mine.length}
                    </span>
                  ) : null}
                </span>
                <span className="xs-name truncate">{d.name}</span>
              </button>
            );
          })}
          {others.length === 0 && (
            /* Nessun'altra macchina tua: il quadrato è la porta per collegarne una. */
            <button className="xs-cell cold" onClick={() => fire(startPairing("device_host"))}>
              <span className="dev-tile dashed">
                <Icon.Plus size={26} />
              </span>
              <span className="xs-name truncate">{t("exchange.pairTile")}</span>
            </button>
          )}
        </div>
      </div>

      <div className="xs-section">
        <div className="xs-head">
          <span className="xs-head-title">{t("title.people")}</span>
          {picking ? (
            <button className="xs-head-link" onClick={() => setPicking(null)}>
              {t("common.cancel")}
            </button>
          ) : (
            <button className="xs-head-link" onClick={() => push("people")}>
              {t("send.book")}
            </button>
          )}
        </div>
        <div className="xs-grid">
          {faces.map((c) => {
            const online = presence[c.id] === true;
            const shown = c.display_name || c.name;
            return (
              <FaceCell
                key={c.name}
                name={shown}
                sub={faceSub(c, online, t)}
                online={online}
                picked={picking ? picking.includes(c.name) : undefined}
                onTap={() =>
                  picking ? togglePick(c.name) : startCompose({ kind: "contact", name: c.name })
                }
                onHold={() => {
                  if (picking) return;
                  haptic.lifted();
                  setPicking([c.name]);
                }}
              />
            );
          })}
          {picking || people.length > FACES ? (
            <button className="xs-cell cold" onClick={() => push("people")}>
              <span className="face-pic dashed">
                <Icon.People size={26} />
              </span>
              <span className="xs-name truncate">{t("send.book")}</span>
            </button>
          ) : null}
          {people.length === 0 && !picking ? (
            <button className="xs-cell cold" onClick={() => push("people")}>
              <span className="face-pic dashed">
                <Icon.Plus size={26} />
              </span>
              <span className="xs-name truncate">{t("send.book")}</span>
            </button>
          ) : null}
        </div>
      </div>

      {picking ? (
        /* 16a: quante, e come arriveranno, prima di scegliere i file. */
        <PickBar
          names={picking}
          online={pickedOnline}
          onGo={() => {
            const names = picking;
            setPicking(null);
            startCompose({ kind: "group", names });
          }}
        />
      ) : null}
    </div>
  );
}

/** A face that is a button, with a hold on it: the hold turns the faces into
 *  boxes (16a), and the tap that would follow the hold is swallowed. */
function FaceCell({
  name,
  sub,
  online,
  picked,
  onTap,
  onHold,
}: {
  name: string;
  sub: string;
  online: boolean;
  picked?: boolean;
  onTap: () => void;
  onHold: () => void;
}) {
  const timer = useRef<number>();
  const held = useRef(false);
  const arm = () => {
    held.current = false;
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      held.current = true;
      onHold();
    }, HOLD_MS);
  };
  const disarm = () => window.clearTimeout(timer.current);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return (
    <button
      className="xs-cell"
      aria-pressed={picked}
      onPointerDown={arm}
      onPointerUp={disarm}
      onPointerLeave={disarm}
      onPointerCancel={disarm}
      onContextMenu={(e) => e.preventDefault()}
      onClick={() => {
        if (held.current) {
          held.current = false;
          return;
        }
        onTap();
      }}
    >
      <Face name={name} online={online} picked={picked} />
      <span style={{ minWidth: 0, maxWidth: "100%" }}>
        <span className="xs-name truncate" style={{ display: "block" }}>
          {name}
        </span>
        {sub ? (
          <span className="xs-sub truncate" style={{ display: "block" }}>
            {sub}
          </span>
        ) : null}
      </span>
    </button>
  );
}

/** La barra blu in fondo mentre si scelgono le persone (16a). Nel dock, sopra
 *  la barra, dove sta anche la scheda del trasferimento. */
function PickBar({ names, online, onGo }: { names: string[]; online: number; onGo: () => void }) {
  const t = useT();
  const contacts = useStore((s) => s.contacts);
  const shown = (n: string) => contacts.find((c) => c.name === n)?.display_name || n;
  // Sopra la barra, fuori dalla pagina che scorre: è dove sta anche la scheda
  // del trasferimento, e i due si impilano.
  return createPortal(
    <div className="dock">
      <div className="xfer-card sel" role="status">
        <span className="xfer-pile" aria-hidden>
          {names.slice(0, 3).map((n, i) => (
            <span key={n} className="sel-av" style={{ zIndex: 10 - i }}>
              {initial(shown(n))}
            </span>
          ))}
        </span>
        <div className="grow" style={{ minWidth: 0 }}>
          <div className="xfer-title tnum">{t("exchange.pickedCount", names.length)}</div>
          <div className="xfer-sub tnum">
            <span>{t("exchange.pickedSub", online, names.length - online)}</span>
          </div>
        </div>
        <button className="sel-go" disabled={names.length === 0} onClick={onGo}>
          {t("exchange.next")} <Icon.ChevronRight size={14} />
        </button>
      </div>
    </div>,
    document.body
  );
}

/** «dalle 9:30»: the clock the offer arrived at, in the reader's locale. */
function clockOf(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

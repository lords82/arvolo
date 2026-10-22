// Cosa mandi: la seconda domanda di Scambia, a tutto schermo (14b).
//
// La prima domanda (`ExchangeView`) ha già risposto *con chi*; qui si risponde
// *cosa* e si parte. Chi resta in una capsula in alto, con l'anello se è
// collegata e l'icona che apre le opzioni della casella (13d). Sotto, tre
// chip — Foto, File, Appunti — e la galleria che occupa tutto: le foto si
// numerano nell'ordine in cui le tocchi. Il foglio blu notte in fondo raccoglie
// quanto parte, come arriva, il messaggio e il bottone rotondo d'invio.
//
// Il trasporto lo decide l'app, fra quelli sigillati: una persona riceve adesso
// se è collegata e trova il file nella sua casella se no; una tua macchina
// prende senza che nessuno accetti; più persone (16a) sono un invio per
// ciascuna. Le opzioni (13d) sono le regole della casella — quanto resta, quanti
// ritiri, una password — e l'eccezione a parte: aspettare che sia collegata,
// senza lasciare niente sul relay. Il link è l'unica cosa pubblica.

import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api } from "../ipc";
import { failText, fmtBytes, isMedia, waitingText } from "../format";
import { useT } from "../i18n";
import { SELF_RECIPIENT, useStore, type ComposeTo } from "../store";
import type { PickedItem, RecentPhoto } from "../types";
import { Progress, useCopy } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Switch } from "../ui/Primitives";
import { toast } from "../ui/Toasts";
import { haptic } from "../haptics";
import { phoneOs } from "../phone";
import { deviceGlyph, initial, isAwake } from "./ExchangeView";

/** Quante foto per pagina. La galleria scorre, e la pagina dopo si chiede
 *  quando si arriva in fondo a quella che c'è: una libreria di diecimila foto
 *  non entra in una chiamata sola — ogni tessera è un JPEG decodificato e
 *  spedito alla webview — e non deve. */
const PAGE = 60;
/** Le quattro durate di 13d, in secondi. */
const TTLS = [3600, 86400, 7 * 86400, 30 * 86400] as const;
const DEFAULT_TTL = 7 * 86400;
const DEFAULT_MAX = 1;

type Tab = "photos" | "files";

/** «Marco, Giulia e Paolo»: la lista come la unisce la lingua in cui si sta
 *  leggendo, e una virgola in meno di quante ne metterebbe un `join`.
 *
 *  `Intl.ListFormat` è ES2021 e il `lib` di questo progetto è ES2020 — quindi è
 *  letto a mano invece che bumpare il target per una riga. A runtime c'è da
 *  iOS 14 e da Chrome 72, cioè ovunque questa app giri; il ramo di scorta è per
 *  il motore che non ce l'ha, non per il caso normale. */
function joinNames(names: string[], tag: string): string {
  const LF = (Intl as unknown as {
    ListFormat?: new (
      locale: string,
      opts: { type: "conjunction" }
    ) => { format(items: string[]): string };
  }).ListFormat;
  if (!LF) return names.join(", ");
  try {
    return new LF(tag, { type: "conjunction" }).format(names);
  } catch {
    return names.join(", ");
  }
}

/** Il nome come sta in rubrica, intero.
 *
 *  C'è stata per mezz'ora una funzione che ne prendeva la prima parola — «Manda
 *  a Marco» invece di «Manda a Marco Rinaldi», che è quello che il canvas
 *  disegna perché il suo esempio si chiama Sara. Su «Studio Neri» dava «Studio»,
 *  e su una rubrica vera i nomi di studi, ditte e gruppi sono tanti quanti
 *  quelli di persona. Un nome tagliato dai puntini si capisce; un nome *altro*
 *  no — e il taglio è un fatto della larghezza del bottone, che cambia con la
 *  lingua e col corpo del testo, non del nome. */
const short = (full: string) => full;

/** Le regole della casella (13d). I default sono quelli del daemon. */
interface Opts {
  ttl: number;
  max: number;
  password: string;
  /** «Aspetta che sia collegata»: niente sul relay, mai. */
  wait: boolean;
}
const DEFAULT_OPTS: Opts = { ttl: DEFAULT_TTL, max: DEFAULT_MAX, password: "", wait: false };
const isDefaultMailbox = (o: Opts) =>
  o.ttl === DEFAULT_TTL && o.max === DEFAULT_MAX && !o.password;

export function ComposeView() {
  const t = useT();
  const to = useStore((s) => s.composeTo);
  const shared = useStore((s) => s.composeFiles);
  const contacts = useStore((s) => s.contacts);
  const devices = useStore((s) => s.devices);
  const presence = useStore((s) => s.presence);
  const send = useStore((s) => s.send);
  const deposit = useStore((s) => s.deposit);
  const link = useStore((s) => s.link);
  const codeOf = useStore((s) => s.code);
  const clearCompose = useStore((s) => s.clearCompose);
  const back = useStore((s) => s.back);
  const go = useStore((s) => s.go);

  const [picked, setPicked] = useState<PickedItem[]>(shared);
  const [tiles, setTiles] = useState<RecentPhoto[] | null | undefined>(undefined);
  /** In the order they were tapped: the number on the tile is that order. */
  const [chosen, setChosen] = useState<string[]>([]);
  const [tab, setTab] = useState<Tab>("photos");
  const [note, setNote] = useState("");
  const [opts, setOpts] = useState<Opts>(DEFAULT_OPTS);
  const [optsOpen, setOptsOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  /** The link just started: its URL once it has one, and the transfer
   *  uploading it — the URL lands on that row. */
  const [ready, setReady] = useState<{ url: string; transfer: number | null } | null>(null);
  /** 17e: il campo ha il fuoco. La griglia si ritira e il foglio sale con la
   *  tastiera — `--kb` fa già lo spazio, qui si decide chi lo occupa. */
  const [writing, setWriting] = useState(false);
  /** C'è altro oltre l'ultima pagina chiesta. */
  const [more, setMore] = useState(false);
  const loadingMore = useRef(false);

  // What the phone has to offer: the last pictures, and whatever was copied
  // last. Both asked once on arrival; a refused library simply leaves no grid,
  // and the files chip takes the front.
  useEffect(() => {
    api
      .photosRecent(PAGE)
      .then((p) => {
        setTiles(p);
        setMore(!!p && p.length === PAGE);
        if (!p || !p.length) setTab("files");
      })
      .catch(() => {
        setTiles(null);
        setTab("files");
      });
  }, []);

  /** La pagina dopo, quando la griglia arriva in fondo. Una alla volta: due
   *  chiamate insieme tornerebbero le stesse foto e le mostrerebbero due
   *  volte. */
  const loadMore = () => {
    if (!tiles || !more || loadingMore.current) return;
    loadingMore.current = true;
    api
      .photosRecent(PAGE, tiles.length)
      .then((p) => {
        if (p && p.length) {
          const seen = new Set(tiles.map((x) => x.id));
          setTiles([...tiles, ...p.filter((x) => !seen.has(x.id))]);
        }
        setMore(!!p && p.length === PAGE);
      })
      .catch(() => setMore(false))
      .finally(() => {
        loadingMore.current = false;
      });
  };
  // Arrived with files in hand (Condividi): they are the point, show them.
  useEffect(() => {
    if (shared.length) setTab("files");
  }, [shared.length]);

  const contact = to?.kind === "contact" ? contacts.find((c) => c.name === to.name) : null;
  const device = to?.kind === "self" ? devices.find((d) => d.name === to.device) : null;
  const group = to?.kind === "group" ? to.names.map((n) => contacts.find((c) => c.name === n) ?? { name: n }) : [];
  const online = (id?: string) => !!id && presence[id] === true;
  /** Come si chiama chi riceve, in una parola sola. Calcolato qui e non più in
   *  fondo perché ora lo legge anche la riga «Sigillato per …». */
  const name = !to
    ? ""
    : to.kind === "contact"
      ? contact?.display_name || to.name
      : to.kind === "self"
        ? to.device
        : to.kind === "group"
          ? t("compose.groupName", to.names.length)
          : to.kind === "code"
            ? t("send.modeCode")
            : t("send.publicWord");
  const contactOnline = !!contact && online(contact.id);
  const groupOnline = group.filter((c) => "id" in c && online(c.id)).length;

  // Foto e file sono **una scelta sola** (17a/17b): i chip sono tre sorgenti,
  // non tre invii, e il conto in fondo li somma. Prima vivevano in due stati
  // che nessuno sommava se non per il totale — qui diventano una lista, che è
  // anche ciò che serve per disegnarne i quadratini e toglierne uno.
  const inHand = useMemo(() => {
    const fromTiles = chosen.map((id) => {
      const p = tiles?.find((x) => x.id === id);
      return {
        id,
        name: p?.name ?? "",
        thumb: p?.thumb,
        isVideo: !!p?.isVideo,
        photo: true,
      };
    });
    const fromFiles = picked.map((f) => ({
      id: f.id,
      name: f.name,
      thumb: undefined as string | undefined,
      isVideo: false,
      photo: false,
    }));
    return [...fromTiles, ...fromFiles];
  }, [chosen, picked, tiles]);
  const count = inHand.length;
  // Il titolo del foglio dice *di che cosa* si tratta quando si può: tre foto
  // sono «3 foto», due foto e un video sono «2 foto, 1 video», e appena c'è
  // dentro anche un file diventano «3 elementi» — perché non c'è un nome che
  // li copra tutti senza mentire su uno.
  const what = useMemo(() => {
    const vids = inHand.filter((x) => x.photo && x.isVideo).length;
    const pics = inHand.filter((x) => x.photo && !x.isVideo).length;
    const files = inHand.filter((x) => !x.photo).length;
    if (files && (pics || vids)) return t("compose.countMixed", count);
    if (files) return picked.every((f) => isMedia(f.name))
      ? t("compose.countPhotos", files)
      : t("compose.countFiles", files);
    if (pics && vids) return t("compose.countPhotoVideo", pics, vids);
    if (vids) return t("compose.countVideos", vids);
    return t("compose.countPhotos", pics);
  }, [inHand, count, picked, t]);
  // The size is only known for what is already a file; a tile is exported
  // when it is sent. So the line says bytes only when it can say all of them.
  const bytes = chosen.length ? 0 : picked.reduce((n, f) => n + (f.size ?? 0), 0);

  /** Togliere un quadratino dal foglio: la foto torna non scelta, il file esce
   *  dalla mano. È la sola via per disfare una scelta senza tornare alla
   *  sorgente da cui veniva. */
  const drop = (id: string, photo: boolean) => {
    haptic.picked();
    if (photo) setChosen((c) => c.filter((x) => x !== id));
    else setPicked((f) => f.filter((x) => x.id !== id));
  };

  const pickFiles = async () => {
    try {
      const got = await api.pickFiles(false, false);
      if (got.length) add(got);
    } catch (e) {
      toast.bad(t("send.pickFailed"), String(e));
    }
  };
  const add = (got: PickedItem[]) =>
    setPicked((f) => {
      const seen = new Set(f.map((x) => x.id));
      return [...f, ...got.filter((x) => !seen.has(x.id))];
    });
  const toggle = (id: string) => {
    haptic.picked();
    setChosen((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]));
  };

  /** Everything in hand as files the engine can take: the tiles are exported
   *  only now, because an export is a copy and most tiles are never sent. */
  const collect = async (): Promise<PickedItem[]> => {
    const fromTiles = chosen.length ? await api.photosExport(chosen) : [];
    return [...picked, ...fromTiles];
  };

  /** Cosa succederà: **una riga per ogni cosa diversa che succede**, non una
   *  sola che le riassuma tutte.
   *
   *  Era una riga, e per un gruppo diceva «2 ora, 1 in casella»: un conteggio,
   *  cioè la forma in cui una frase smette di dire *a chi*. Il canvas (17b) le
   *  separa e le nomina — «Sara e Marco lo ricevono adesso», «Paolo lo trova
   *  nella sua casella» — perché sono due destini diversi dello stesso invio e
   *  chi manda vuole sapere quale tocca a chi. Per una persona sola le righe
   *  sono due lo stesso (17a): come arriva, e che è sigillato per lei. */
  const hows = (): { icon: JSX.Element; text: string }[] => {
    if (!to) return [];
    if (to.kind === "link") return [{ icon: <Icon.Globe size={13} />, text: t("compose.howLink") }];
    if (to.kind === "code") return [{ icon: <Icon.Qr size={13} />, text: t("compose.howCode") }];
    if (to.kind === "self")
      return [{ icon: <Icon.Bolt size={13} />, text: t("compose.howSelf", to.device) }];
    if (opts.wait) return [{ icon: <Icon.Clock size={13} />, text: t("compose.howWait") }];
    if (to.kind === "group") {
      const on = group.filter((c) => "id" in c && online(c.id));
      const off = group.filter((c) => !("id" in c) || !online(c.id));
      // «Marco, Giulia e Paolo», non «Marco Rinaldi, Giulia Bianchi, Studio
      // Neri»: nomi come li si direbbe, e uniti come li unisce la lingua in cui
      // si sta leggendo — in una riga sola che deve starci.
      const say = (xs: typeof group) =>
        joinNames(xs.map((c) => short(("display_name" in c && c.display_name) || c.name)), t("locale.tag"));
      const out: { icon: JSX.Element; text: string }[] = [];
      if (on.length)
        out.push({ icon: <Icon.Bolt size={13} />, text: t("compose.howGroupNow", say(on)) });
      if (off.length)
        out.push({ icon: <Icon.Inbox size={13} />, text: t("compose.howGroupBox", say(off)) });
      return out;
    }
    const first = !isDefaultMailbox(opts)
      ? { icon: <Icon.Inbox size={13} />, text: t("compose.howMailbox", ttlLabel(opts.ttl, t)) }
      : contactOnline
        ? { icon: <Icon.Bolt size={13} />, text: t("compose.howDirect") }
        : { icon: <Icon.Inbox size={13} />, text: t("compose.howMailbox", ttlLabel(DEFAULT_TTL, t)) };
    return [first, { icon: <Icon.Lock size={13} />, text: t("compose.sealedFor", name) }];
  };

  /** One send, the way the options say. Returns when the engine has taken it. */
  const sendOne = async (who: string, ids: string[], device?: string | null) => {
    if (opts.wait && !device) {
      await send(who, ids, note, null, true);
    } else if (!isDefaultMailbox(opts)) {
      await deposit(who, ids, note, opts.ttl, opts.max, opts.password || null, device ?? null);
    } else {
      await send(who, ids, note, device ?? null);
    }
  };

  const goSend = async () => {
    if (!to || count === 0 || busy) return;
    setBusy(true);
    try {
      const files = await collect();
      const ids = files.map((f) => f.id);
      if (to.kind === "link") {
        if (files.length > 1) {
          toast.info(t("send.linkTooMany"));
          return;
        }
        const started = await link(files[0].id, opts.ttl, opts.max === DEFAULT_MAX ? null : opts.max);
        setReady({ url: started.url, transfer: started.id });
        return;
      }
      if (to.kind === "code") {
        // Un codice non parte: resta acceso qui, e la schermata che lo mostra è
        // il codice stesso. `keep` falso — il codice vive quanto il giro che lo
        // sta consegnando, che è quello che la schermata Codici racconta.
        const made = await codeOf(ids, false);
        clearCompose();
        go("codes");
        void made;
        return;
      }
      if (to.kind === "self") {
        await sendOne(SELF_RECIPIENT, ids, to.device);
        toast.echo(t("send.onItsWaySelf"), t("send.onItsWaySelfDetail"));
      } else if (to.kind === "group") {
        for (const n of to.names) await sendOne(n, ids);
        toast.echo(t("compose.onItsWayGroup", to.names.length));
      } else if (to.kind === "contact") {
        await sendOne(to.name, ids);
        toast.echo(t("send.onItsWay", contact?.display_name || to.name), t("send.onItsWayDetail"));
      }
      haptic.did();
      // Dopo «Invia» si torna dove si era (15): la scheda sopra la barra dice
      // il resto.
      clearCompose();
      go("exchange");
    } catch {
      // `act` in the store already recorded the reason and raised the toast.
    } finally {
      setBusy(false);
    }
  };

  if (!to) return null;

  const h = hows();
  const sub =
    to.kind === "contact"
      ? [contactOnline ? t("exchange.online") : null, t(contact?.verified ? "compose.verified" : "compose.unverified")]
          .filter(Boolean)
          .join(" · ")
      : to.kind === "self"
        ? t("exchange.deviceOfYours")
        : to.kind === "group"
          ? t("exchange.pickedSub", groupOnline, group.length - groupOnline)
          : to.kind === "code"
            ? t("send.blurbCode")
            : t("compose.publicSub");

  const leave = () => {
    clearCompose();
    back();
  };

  return (
    <div className={`ios-page compose${writing ? " writing" : ""}`}>
      {/* Chi: la capsula in alto. */}
      <div className="comp-head">
        <button className="comp-back" aria-label={t("common.back")} onClick={leave}>
          <Icon.ChevronLeft size={20} />
        </button>
        <div className="comp-cap">
          {to.kind === "contact" ? (
            <span className="comp-av" aria-hidden>
              <span className={`comp-av-ring${contactOnline ? " on" : ""}`}>
                <span className="comp-av-in">{initial(name)}</span>
              </span>
              {contactOnline ? <span className="face-dot" /> : null}
            </span>
          ) : to.kind === "group" ? (
            <span className="comp-av pile" aria-hidden>
              {group.slice(0, 3).map((c) => (
                <span key={c.name} className={`comp-av-ring${"id" in c && online(c.id) ? " on" : ""}`}>
                  <span className="comp-av-in">{initial(("display_name" in c && c.display_name) || c.name)}</span>
                </span>
              ))}
            </span>
          ) : (
            <span className="comp-av" aria-hidden>
              {to.kind === "self" ? (
                <span className={`comp-av-ring${device && isAwake(device) ? " on" : ""}`}>
                  <span className="comp-av-in">{deviceGlyph(device?.platform ?? "", 20)}</span>
                </span>
              ) : (
                /* 18b/18c: pieno, non un anello intorno al vuoto. Un anello è
                   il posto dove sta una faccia, e qui non c'è nessuna faccia da
                   mettere — il destinatario è «chiunque». */
                <span className="comp-av-fill">
                  {to.kind === "code" ? <Icon.Qr size={20} /> : <Icon.Globe size={20} />}
                </span>
              )}
            </span>
          )}
          <div className="grow" style={{ minWidth: 0 }}>
            <div
              className={`comp-name truncate${to.kind === "link" || to.kind === "code" ? " shout" : ""}`}
            >
              {name}
            </div>
            <div className="comp-sub truncate">{sub}</div>
          </div>
          {/* Niente pulsante qui. Le opzioni si aprono dalla pastiglia della
              scadenza, in fondo (17a/17d): due porte per la stessa stanza — una
              in cima e una in fondo alla stessa schermata — sono una porta di
              troppo, e quella in cima era la meno visibile delle due. */}
        </div>
      </div>

      {/* Da dove: tre chip, e su ciascuno quanto se n'è preso.
          Il contatore è la cosa che rende le tre sorgenti **una scelta sola**
          (17a): si prendono due foto, si passa a File e se ne aggiunge uno, e
          il chip che si è lasciato continua a dire «2». Prima il numero stava
          solo su File ed era testo attaccato all'etichetta — a griglia aperta
          non c'era modo di sapere se nella mano ci fosse già qualcos'altro. */}
      <div className="comp-chips" role="tablist">
        {tiles === null ? null : (
          <button role="tab" className="comp-chip" aria-pressed={tab === "photos"} onClick={() => setTab("photos")}>
            {t("compose.photos")}
            {chosen.length ? <span className="comp-n tnum">{chosen.length}</span> : null}
          </button>
        )}
        <button role="tab" className="comp-chip" aria-pressed={tab === "files"} onClick={() => setTab("files")}>
          {t("compose.file")}
          {picked.length ? <span className="comp-n tnum">{picked.length}</span> : null}
        </button>
      </div>

      {tab === "photos" ? (
        tiles && tiles.length ? (
          <div
            className="tiles"
            role="group"
            aria-label={t("compose.recent")}
            onScroll={(e) => {
              const el = e.currentTarget;
              // Una schermata prima della fine: la pagina arriva mentre si
              // scorre, non dopo essersi fermati sul vuoto.
              if (el.scrollTop + el.clientHeight * 2 >= el.scrollHeight) loadMore();
            }}
          >
            {tiles.map((p) => {
              const n = chosen.indexOf(p.id);
              return (
                <button
                  key={p.id}
                  className={`tile${n >= 0 ? " on" : ""}`}
                  aria-pressed={n >= 0}
                  aria-label={p.name}
                  onClick={() => toggle(p.id)}
                >
                  <img src={p.thumb} alt="" loading="lazy" />
                  {p.isVideo ? <span className="tile-dur tnum">{fmtDur(p.duration)}</span> : null}
                  <span className="tile-mark tnum" aria-hidden>
                    {n >= 0 ? n + 1 : ""}
                  </span>
                </button>
              );
            })}
          </div>
        ) : (
          <div className="comp-list">
            <div className="comp-empty">{tiles === undefined ? "" : t("compose.noPhotos")}</div>
          </div>
        )
      ) : tab === "files" ? (
        <div className="comp-list">
          <div className="ios-group">
            {picked.map((f) => (
              <div key={f.id} className="ios-row" style={{ cursor: "default" }}>
                <span className="grow truncate">{f.name}</span>
                <span className="ios-detail tnum">{fmtBytes(f.size ?? 0)}</span>
                <button
                  className="icon-btn"
                  aria-label={t("send.filesRemove", f.name)}
                  onClick={() => setPicked((p) => p.filter((x) => x.id !== f.id))}
                >
                  <Icon.Close size={14} />
                </button>
              </div>
            ))}
            <button className="ios-row" onClick={() => void pickFiles()}>
              <Icon.Folder size={18} />
              <span className="grow">{t("compose.pickFile")}</span>
              <Icon.ChevronRight size={16} className="ios-chev" />
            </button>
          </div>
          {picked.length === 0 ? <div className="comp-empty">{t("compose.nothingPicked")}</div> : null}
        </div>
      ) : null}

      {/* Il foglio blu notte in fondo (17a–17e).
          Non è un pannello che riassume e basta: è il posto da cui si manda, e
          tutto quello che si può ancora decidere prima di farlo sta **dentro di
          lui**. Quando si tocca «7 giorni» non si apre un dialogo sopra — il
          foglio si allunga (17d); quando si scrive il messaggio non si apre una
          schermata — il foglio sale e la griglia si ritira (17e). Un dialogo
          sopra avrebbe coperto proprio la cosa che si stava guardando per
          decidere: quello che si è scelto. */}
      <div className="comp-foot">
        <div className={`comp-card${optsOpen ? " open" : ""}`}>
          {optsOpen ? (
            <Options
              to={to}
              name={name}
              opts={opts}
              onChange={setOpts}
              onDone={() => setOptsOpen(false)}
            />
          ) : (
            <>
              {count === 0 ? (
                /* 17c: il foglio resta al suo posto e dice cosa fare. Era una
                   riga di testo dove sta il conto; un quadrato tratteggiato
                   dice *dove* finirà quello che si tocca. */
                <div className="comp-none">
                  <span className="comp-none-box" aria-hidden />
                  <span className="comp-none-say">{t("compose.pickHint")}</span>
                </div>
              ) : (
                <>
                  <div className="comp-what">
                    <div className="comp-thumbs">
                      {inHand.slice(0, 3).map((x) => (
                        <span key={x.id} className="comp-thumb">
                          {x.thumb ? (
                            <img src={x.thumb} alt="" />
                          ) : (
                            <Icon.Folder size={18} />
                          )}
                          <button
                            className="comp-thumb-x"
                            aria-label={t("compose.removeItem", x.name)}
                            onClick={() => drop(x.id, x.photo)}
                          >
                            <Icon.Close size={10} />
                          </button>
                        </span>
                      ))}
                      {count > 3 ? <span className="comp-more tnum">+{count - 3}</span> : null}
                    </div>
                    <div className="grow" style={{ minWidth: 0 }}>
                      <div className="comp-count tnum">{what}</div>
                      {bytes ? <div className="comp-size tnum">{fmtBytes(bytes)}</div> : null}
                    </div>
                  </div>
                  <div className="comp-hows">
                    {h.map((x, i) => (
                      <div key={i} className="comp-how">
                        {x.icon}
                        <span className="truncate">{x.text}</span>
                      </div>
                    ))}
                  </div>
                </>
              )}

              {to.kind === "contact" || to.kind === "group" ? (
                <textarea
                  className={`comp-msg${writing ? " big" : ""}`}
                  value={note}
                  rows={1}
                  onChange={(e) => setNote(e.target.value)}
                  onFocus={() => setWriting(true)}
                  onBlur={() => setWriting(false)}
                  placeholder={t("compose.message")}
                  aria-label={t("compose.message")}
                />
              ) : null}

            </>
          )}

          <div className="comp-row">
            {writing ? (
              /* 17e: mentre si scrive, al posto delle opzioni c'è la sola
                 cosa che riguarda il messaggio — che parte sigillato con i
                 file e non per conto suo. */
              <span className="comp-seal">
                <Icon.Lock size={13} />
                <span className="truncate">{t("compose.sealedTravels")}</span>
              </span>
            ) : optsOpen || to.kind === "self" || to.kind === "code" ? null : (
              <button
                className="comp-ttl"
                aria-label={t("compose.optionsOpen")}
                onClick={() => setOptsOpen(true)}
              >
                <Icon.Clock size={14} />
                {ttlLabel(opts.ttl, t)}
              </button>
            )}
            <button
              className={`comp-go${busy ? " busy" : ""}${writing ? " small" : ""}`}
              disabled={count === 0 || busy}
              onClick={() => void goSend()}
            >
              {busy ? (
                <Icon.Refresh size={18} />
              ) : to.kind === "link" ? (
                <Icon.Link size={18} />
              ) : (
                <Icon.ArrowUpRight size={18} />
              )}
              <span className="truncate">
                {writing
                  ? t("compose.go")
                  : to.kind === "link"
                    ? t("compose.goLink")
                    : to.kind === "self"
                      ? t("compose.goSelf", to.device)
                      : to.kind === "group"
                    ? t("compose.goGroup")
                    : t("compose.goTo", short(name))}
              </span>
            </button>
          </div>
        </div>
      </div>

      {ready && (
        <LinkReady
          url={ready.url}
          transfer={ready.transfer}
          what={bytes ? `${what} · ${fmtBytes(bytes)}` : what}
          onDone={() => {
            setReady(null);
            clearCompose();
            go("transfers");
          }}
        />
      )}
    </div>
  );
}

function ttlLabel(secs: number, t: ReturnType<typeof useT>): string {
  const i = TTLS.indexOf(secs as (typeof TTLS)[number]);
  return t(["send.ttl1h", "send.ttl1d", "send.ttl7d", "send.ttl30d"][i < 0 ? 2 : i] as "send.ttl7d");
}

/** Le opzioni di consegna (17d), **dentro il foglio** e non sopra di esso.
 *
 *  Erano un dialogo a parte, su uno scrim, aperto dalla capsula in alto. Due
 *  cose non andavano. La prima è che copriva quello che si stava guardando per
 *  decidere — le foto scelte — e la domanda «quanto deve restare in casella?»
 *  si risponde guardando *cosa* ci si mette. La seconda è che un dialogo è un
 *  posto, e tornava indietro: si apriva, si decideva, si chiudeva, e solo
 *  allora si mandava. Qui il foglio si allunga, il bottone d'invio resta in
 *  fondo dov'era, e si manda **da qui** senza chiudere niente.
 *
 *  L'ordine è quello del canvas: la scadenza in cima, perché è l'unica che si
 *  cambia spesso; l'eccezione — aspettare che sia collegata, senza lasciare
 *  niente sul relay — in fondo, perché spegne tutto il resto. */
function Options({
  to,
  name,
  opts,
  onChange,
  onDone,
}: {
  to: ComposeTo;
  name: string;
  opts: Opts;
  onChange: (o: Opts) => void;
  onDone: () => void;
}) {
  const t = useT();
  const [pw, setPw] = useState(false);
  const person = to.kind === "contact" || to.kind === "group";
  return (
    <>
      <span className="comp-grab" aria-hidden />
      <div className="opt-head">
        <span className="opt-title truncate">{t("compose.optionsTitle", short(name))}</span>
        <button className="opt-done" onClick={onDone}>
          {t("common.done")}
        </button>
      </div>

      {/* La riga che dice a cosa serve la scadenza, e poi la scadenza. */}
      <div className="opt-ttl">
        <div className="opt-ttl-say">
          <Icon.Inbox size={13} />
          <span>{t("compose.noteContact")}</span>
        </div>
        <div className="opt-seg" role="radiogroup" aria-label={t("compose.ttlLabel")}>
          {TTLS.map((secs) => (
            <button
              key={secs}
              role="radio"
              aria-checked={opts.ttl === secs}
              onClick={() => {
                haptic.picked();
                onChange({ ...opts, ttl: secs });
              }}
            >
              {ttlLabel(secs, t)}
            </button>
          ))}
        </div>
      </div>

      <div className="opt-rows">
        <div className="opt-row">
          <Icon.Download size={18} />
          <div className="grow" style={{ minWidth: 0 }}>
            <div className="opt-label">{t("compose.pickups")}</div>
            <div className="opt-hint tnum">{t("compose.pickupsHint", opts.max)}</div>
          </div>
          <div className="stepper" role="group" aria-label={t("compose.pickups")}>
            <button
              aria-label={t("compose.fewer")}
              disabled={opts.max <= 1}
              onClick={() => onChange({ ...opts, max: Math.max(1, opts.max - 1) })}
            >
              −
            </button>
            <i className="tnum">{opts.max}</i>
            <button
              aria-label={t("compose.more")}
              disabled={opts.max >= 99}
              onClick={() => onChange({ ...opts, max: Math.min(99, opts.max + 1) })}
            >
              +
            </button>
          </div>
        </div>

        {to.kind !== "link" ? (
          <div className="opt-row">
            <Icon.Lock size={18} />
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="opt-label">{t("compose.password")}</div>
              <div className="opt-hint">{t("compose.passwordHint")}</div>
            </div>
            {pw ? (
              <span className="opt-value">
                <input
                  type="password"
                  autoComplete="new-password"
                  autoFocus
                  value={opts.password}
                  onChange={(e) => onChange({ ...opts, password: e.target.value })}
                  onBlur={() => setPw(false)}
                  placeholder={t("compose.none")}
                  aria-label={t("compose.password")}
                />
              </span>
            ) : (
              <button className="opt-value" onClick={() => setPw(true)}>
                {opts.password ? "••••••" : t("compose.none")}
                <Icon.ChevronRight size={14} />
              </button>
            )}
          </div>
        ) : null}

        {person ? (
          <div className="opt-row">
            <Icon.Bolt size={18} />
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="opt-label">{t("compose.waitTitle")}</div>
              <div className="opt-hint">{t("compose.waitDesc")}</div>
            </div>
            <Switch
              checked={opts.wait}
              onChange={(v) => onChange({ ...opts, wait: v })}
              label={t("compose.waitTitle")}
            />
          </div>
        ) : null}
      </div>
    </>
  );
}

/** Link pronto (18c): un foglio dal basso, non un dialogo al centro.
 *
 *  È il momento in cui l'unica cosa che conta è l'indirizzo appena nato, e le
 *  due cose che se ne fanno: copiarlo o passarlo a qualcuno. Prima era un
 *  pannello centrato con il quadrato del QR dentro — ma un link pubblico non si
 *  inquadra, si manda; il quadrato serviva a un codice e qui era un'abitudine
 *  ereditata.
 *
 *  «Condividi» apre il foglio di sistema e c'è solo dove quel foglio esiste:
 *  oggi iOS. Su Android `ACTION_SEND` è ancora da scrivere in Kotlin, e un
 *  bottone che non fa niente è peggio di un bottone che non c'è. */
function LinkReady({
  url: given,
  transfer,
  what,
  onDone,
}: {
  url: string;
  /** The transfer uploading the link, when it is still going up. */
  transfer: number | null;
  what: string;
  onDone: () => void;
}) {
  const t = useT();
  const { state, copy } = useCopy();
  // Until the relay has the file there is no address to hand over: the sheet
  // shows the upload instead, and turns into «Link pronto» when its row gets
  // the URL. Closing it in between only stops watching.
  const tx = useStore((s) => (transfer != null ? s.transfers[`t${transfer}`] : undefined));
  const cancel = useStore((s) => s.cancel);
  const url = given || tx?.link || "";
  const failed = tx?.status === "failed" || tx?.status === "cancelled";
  // Tutti e due i telefoni, adesso: iOS col suo `UIActivityViewController`,
  // Android con `ACTION_SEND`. La finestra no — lì un link si copia.
  const canShare = phoneOs() !== "other";
  return createPortal(
    <>
      <div className="scrim" onClick={onDone} />
      <div
        className="ready"
        role="dialog"
        aria-modal="true"
        aria-label={t("compose.linkReady")}
      >
        <span className="comp-grab" aria-hidden />
        <div className="ready-head">
          <span className="ready-ico" aria-hidden>
            <Icon.Link size={26} />
          </span>
          <div className="ready-title">
            {url
              ? t("compose.linkReady")
              : failed
                ? t("send.uploadFailedTitle")
                : t(
                    "send.uploadingTitle",
                    fmtBytes(tx?.transferred ?? 0),
                    fmtBytes(tx?.size ?? 0)
                  )}
          </div>
          <div className="ready-what tnum">{what}</div>
        </div>
        {url ? (
          <div className="ready-url mono">{url}</div>
        ) : tx && !failed ? (
          <>
            <Progress t={tx} />
            {tx.status === "stalled" && (
              <div className="ready-note">{waitingText(tx)}</div>
            )}
          </>
        ) : tx?.status === "failed" ? (
          <div className="ready-note">{failText(tx.failCode, tx.failDetail, tx.reason)}</div>
        ) : null}
        {url ? (
        <div className="ready-acts">
          <button
            className="ready-btn"
            onClick={() => {
              void copy(url).then(() => haptic.did());
            }}
          >
            {state === "ok" ? <Icon.Check size={17} /> : <Icon.Copy size={17} />}
            {state === "ok" ? t("common.copied") : t("common.copy")}
          </button>
          {canShare ? (
            <button
              className="ready-btn strong"
              onClick={() => {
                api.shareLink(url).catch((e: unknown) =>
                  toast.bad(t("compose.shareFailed"), String(e))
                );
              }}
            >
              <Icon.ArrowUpRight size={17} />
              {t("common.share")}
            </button>
          ) : null}
        </div>
        ) : !failed ? (
          <div className="ready-acts">
            <button
              className="ready-btn"
              onClick={() => {
                if (transfer != null) void cancel(transfer);
              }}
            >
              {t("send.uploadStopAction")}
            </button>
          </div>
        ) : null}
        {url ? (
          <div className="ready-note">{t("compose.linkWhereBack")}</div>
        ) : !failed ? (
          <div className="ready-note">{t("send.uploadingNote")}</div>
        ) : null}
        <button className="ready-done" onClick={onDone}>
          {t("common.done")}
        </button>
      </div>
    </>,
    document.body
  );
}

function fmtDur(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s < 10 ? "0" : ""}${s}`;
}

export type { ComposeTo };

// arvolo secure-download page.
//
// Fetches the encrypted container from the relay and decrypts it *in the
// browser* with WebCrypto (AES-256-GCM), chunk by chunk, mirroring the Rust
// encoder in core/src/link.rs. The key comes only from this URL's #fragment and
// is never sent to the server, so the relay stays zero-knowledge.
//
// Sink strategy (best → fallback):
//   1. File System Access API (Chromium): streams plaintext straight to disk,
//      so nothing but one chunk at a time is ever held in memory.
//   2. Blob (Firefox/Safari/older): buffers the plaintext, then saves. Bounded
//      by the relay's per-blob deposit cap (256 MiB by default).
//
// Container layout (little-endian), see core/src/link.rs:
//   magic "ARVLNK01" | chunk_size u32 | total_size u64 | meta_len u32 |
//   meta_ct[meta_len] | { ct_len u32 | chunk_ct[ct_len] } *
// meta is sealed as the reserved chunk index 0xFFFFFFFF; its plaintext is the
// UTF-8 file name.

"use strict";

const MAGIC = "ARVLNK01";
const META_INDEX = 0xffffffff;

// --- i18n -------------------------------------------------------------------
// The same four languages the desktop app speaks (gui/src/i18n), in one table
// because the page's static labels and its running commentary are the same
// vocabulary and must not drift apart. The HTML ships the English strings
// inline, so they are both the fallback and what a reader sees if this script
// never runs; every one of them carries a `data-t`/`data-html` key below.
//
// There is no language picker: whoever opens a download link is a stranger with
// one visit, and the browser already knows what they read.
const STRINGS = {
  en: {
    title: "arvolo — secure download",
    tag: "Secure download",
    badge: "End-to-end encrypted",
    heading: "Someone sent you a file",
    sub:
      "Your browser decrypts it right here — no install, no account. The relay " +
      "only ever held ciphertext, and never sees the file or the key.",
    fileName: "Encrypted file",
    fileMeta: "Its name and size appear once your browser opens it",
    keyNote:
      "The decryption key lives only in this link's <code>#fragment</code> — the part " +
      "browsers never send to a server.",
    footer:
      'Sent with <a href="https://github.com/lords82/arvolo" target="_blank" ' +
      'rel="noopener noreferrer">arvolo</a> — self-hosted, zero-knowledge file transfer',
    preparing: "Preparing…",
    download: "Download",
    downloading: "Downloading…",
    downloaded: "Downloaded",
    unavailable: "Unavailable",
    ready: "Ready. Nothing is downloaded until you click.",
    fetching: "Fetching encrypted data…",
    toDisk: "Decrypting and saving to disk…",
    toMemory: "Decrypting in memory…",
    done: (size) => "Done — " + size + " decrypted and saved.",
    cancelled: "Save cancelled.",
    compat:
      "<b>This browser isn't fully compatible.</b> It can't stream the download to " +
      "disk, so the file is decrypted in memory — very large files may fail. " +
      "For big files, use Chrome, Edge, or Firefox.",
    errNoCrypto: "This browser can't decrypt here (WebCrypto unavailable — needs HTTPS).",
    errNoKey: "This link is missing its decryption key (the part after #).",
    errBadKey: "This link's key is malformed.",
    errGone:
      "This link has expired or was already downloaded (one-time links burn on first pickup).",
    errNotArvolo: "This isn't an arvolo download link.",
    errDecrypt: "Decryption failed — wrong key or corrupted data.",
    errTruncated: "The download stopped before the end.",
    errGeneric: "Download failed.",
  },
  it: {
    title: "arvolo — download sicuro",
    tag: "Download sicuro",
    badge: "Cifrato end-to-end",
    heading: "Qualcuno ti ha mandato un file",
    sub:
      "Il tuo browser lo decifra qui, senza installare niente e senza account. " +
      "Il relay ha conservato solo byte cifrati: non vede né il file né la chiave.",
    fileName: "File cifrato",
    fileMeta: "Nome e dimensione compaiono quando il browser lo apre",
    keyNote:
      "La chiave di decifratura sta solo dopo il <code>#</code> di questo link — " +
      "la parte che i browser non mandano mai al server.",
    footer:
      'Inviato con <a href="https://github.com/lords82/arvolo" target="_blank" ' +
      'rel="noopener noreferrer">arvolo</a> — trasferimento file self-hosted, a conoscenza zero',
    preparing: "Preparazione…",
    download: "Scarica",
    downloading: "Scaricamento…",
    downloaded: "Scaricato",
    unavailable: "Non disponibile",
    ready: "Pronto. Non scarichi niente finché non premi.",
    fetching: "Recupero dei dati cifrati…",
    toDisk: "Decifratura e salvataggio su disco…",
    toMemory: "Decifratura in memoria…",
    done: (size) => "Fatto — " + size + " decifrati e salvati.",
    cancelled: "Salvataggio annullato.",
    compat:
      "<b>Questo browser non è del tutto compatibile.</b> Non riesce a scrivere il " +
      "download direttamente su disco, quindi il file viene decifrato in memoria: " +
      "i file molto grandi possono non riuscire. Per quelli usa Chrome, Edge o Firefox.",
    errNoCrypto:
      "Questo browser non può decifrare qui (WebCrypto non disponibile — serve HTTPS).",
    errNoKey: "A questo link manca la chiave di decifratura (la parte dopo il #).",
    errBadKey: "La chiave di questo link è malformata.",
    errGone:
      "Questo link è scaduto oppure è già stato scaricato (i link usa e getta si bruciano al primo prelievo).",
    errNotArvolo: "Questo non è un link di download di arvolo.",
    errDecrypt: "Decifratura fallita — chiave sbagliata o dati corrotti.",
    errTruncated: "Il download si è interrotto prima della fine.",
    errGeneric: "Download fallito.",
  },
  fr: {
    title: "arvolo — téléchargement sécurisé",
    tag: "Téléchargement sécurisé",
    badge: "Chiffré de bout en bout",
    heading: "Quelqu'un vous a envoyé un fichier",
    sub:
      "Votre navigateur le déchiffre ici même, sans installation ni compte. Le relais " +
      "n'a gardé que des octets chiffrés : il ne voit ni le fichier ni la clé.",
    fileName: "Fichier chiffré",
    fileMeta: "Le nom et la taille apparaissent quand votre navigateur l'ouvre",
    keyNote:
      "La clé de déchiffrement ne vit qu'après le <code>#</code> de ce lien — la partie " +
      "que les navigateurs n'envoient jamais au serveur.",
    footer:
      'Envoyé avec <a href="https://github.com/lords82/arvolo" target="_blank" ' +
      'rel="noopener noreferrer">arvolo</a> — transfert de fichiers auto-hébergé, à connaissance nulle',
    preparing: "Préparation…",
    download: "Télécharger",
    downloading: "Téléchargement…",
    downloaded: "Téléchargé",
    unavailable: "Indisponible",
    ready: "Prêt. Rien n'est téléchargé tant que vous ne cliquez pas.",
    fetching: "Récupération des données chiffrées…",
    toDisk: "Déchiffrement et enregistrement sur le disque…",
    toMemory: "Déchiffrement en mémoire…",
    done: (size) => "Terminé — " + size + " déchiffrés et enregistrés.",
    cancelled: "Enregistrement annulé.",
    compat:
      "<b>Ce navigateur n'est pas entièrement compatible.</b> Il ne peut pas écrire le " +
      "téléchargement directement sur le disque : le fichier est déchiffré en mémoire, " +
      "et les très gros fichiers peuvent échouer. Pour ceux-là, utilisez Chrome, Edge ou Firefox.",
    errNoCrypto:
      "Ce navigateur ne peut pas déchiffrer ici (WebCrypto indisponible — HTTPS requis).",
    errNoKey: "Il manque à ce lien sa clé de déchiffrement (la partie après le #).",
    errBadKey: "La clé de ce lien est mal formée.",
    errGone:
      "Ce lien a expiré ou a déjà été téléchargé (les liens à usage unique se consument au premier retrait).",
    errNotArvolo: "Ceci n'est pas un lien de téléchargement arvolo.",
    errDecrypt: "Échec du déchiffrement — mauvaise clé ou données corrompues.",
    errTruncated: "Le téléchargement s'est interrompu avant la fin.",
    errGeneric: "Échec du téléchargement.",
  },
  de: {
    title: "arvolo — sicherer Download",
    tag: "Sicherer Download",
    badge: "Ende-zu-Ende-verschlüsselt",
    heading: "Jemand hat Ihnen eine Datei geschickt",
    sub:
      "Ihr Browser entschlüsselt sie direkt hier — ohne Installation, ohne Konto. " +
      "Das Relay hat nur verschlüsselte Bytes verwahrt und sieht weder die Datei " +
      "noch den Schlüssel.",
    fileName: "Verschlüsselte Datei",
    fileMeta: "Name und Größe erscheinen, sobald Ihr Browser sie öffnet",
    keyNote:
      "Der Schlüssel steht nur hinter dem <code>#</code> dieses Links — dem Teil, den " +
      "Browser nie an den Server schicken.",
    footer:
      'Gesendet mit <a href="https://github.com/lords82/arvolo" target="_blank" ' +
      'rel="noopener noreferrer">arvolo</a> — selbst gehosteter Zero-Knowledge-Dateitransfer',
    preparing: "Wird vorbereitet…",
    download: "Herunterladen",
    downloading: "Wird heruntergeladen…",
    downloaded: "Heruntergeladen",
    unavailable: "Nicht verfügbar",
    ready: "Bereit. Es wird nichts geladen, bis Sie klicken.",
    fetching: "Verschlüsselte Daten werden geholt…",
    toDisk: "Wird entschlüsselt und auf die Festplatte geschrieben…",
    toMemory: "Wird im Arbeitsspeicher entschlüsselt…",
    done: (size) => "Fertig — " + size + " entschlüsselt und gespeichert.",
    cancelled: "Speichern abgebrochen.",
    compat:
      "<b>Dieser Browser ist nicht vollständig kompatibel.</b> Er kann den Download " +
      "nicht direkt auf die Festplatte schreiben, die Datei wird also im " +
      "Arbeitsspeicher entschlüsselt — sehr große Dateien können scheitern. Nehmen " +
      "Sie dafür Chrome, Edge oder Firefox.",
    errNoCrypto:
      "Dieser Browser kann hier nicht entschlüsseln (WebCrypto nicht verfügbar — HTTPS nötig).",
    errNoKey: "Diesem Link fehlt sein Schlüssel (der Teil hinter dem #).",
    errBadKey: "Der Schlüssel dieses Links ist fehlerhaft.",
    errGone:
      "Dieser Link ist abgelaufen oder wurde bereits heruntergeladen (Einmal-Links verfallen beim ersten Abholen).",
    errNotArvolo: "Das ist kein arvolo-Download-Link.",
    errDecrypt: "Entschlüsselung fehlgeschlagen — falscher Schlüssel oder beschädigte Daten.",
    errTruncated: "Der Download ist vorzeitig abgebrochen.",
    errGeneric: "Download fehlgeschlagen.",
  },
};

// `navigator.languages` is in the reader's own order of preference, so a browser
// set to French-then-Italian gets French; only one that prefers none of the four
// falls back to English. Same rule as `systemLang()` in gui/src/i18n/index.ts.
function pickLang() {
  const tags = (navigator.languages && navigator.languages.length
    ? navigator.languages
    : [navigator.language]
  ).filter(Boolean);
  for (const tag of tags) {
    const base = String(tag).toLowerCase().split("-")[0];
    if (Object.prototype.hasOwnProperty.call(STRINGS, base)) return base;
  }
  return "en";
}

const LANG = pickLang();
const t = (key, arg) => {
  const v = STRINGS[LANG][key] !== undefined ? STRINGS[LANG][key] : STRINGS.en[key];
  return typeof v === "function" ? v(arg) : v;
};

/** Swap the English mark-up for the reader's language, before first paint: this
 *  script is the last thing in <body> and blocks the parser, so there is no
 *  flash of the fallback. */
function applyI18n() {
  document.documentElement.lang = LANG;
  document.title = t("title");
  if (LANG === "en") return; // the document already says it
  for (const el of document.querySelectorAll("[data-t]")) {
    el.textContent = t(el.dataset.t);
  }
  for (const el of document.querySelectorAll("[data-html]")) {
    el.innerHTML = t(el.dataset.html);
  }
}

const $ = (id) => document.getElementById(id);
const nameEl = $("name"),
  sizeEl = $("size"),
  extEl = $("ext"),
  goEl = $("go"),
  goLabelEl = $("golabel"),
  barEl = $("bar"),
  fillEl = $("fill"),
  statusEl = $("status"),
  warnEl = $("warn");

function showWarn(html) {
  warnEl.innerHTML = html;
  warnEl.hidden = false;
}

function setStatus(msg, cls) {
  statusEl.textContent = msg || "";
  statusEl.className = "status" + (cls ? " " + cls : "");
}

// The button keeps its icon; only the label changes, so this writes to the span
// rather than to the button (whose textContent would take the glyph with it).
function setGo(label, enabled, busy) {
  goLabelEl.textContent = label;
  goEl.disabled = !enabled;
  goEl.classList.toggle("busy", !!busy);
}

function fmtSize(n) {
  const u = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return (i === 0 ? n : n.toFixed(1)) + " " + u[i];
}

// The 36px file chip, same rule as the app's `extOf`: an empty suffix would
// read as a rendering fault, so it falls back to the no-extension label.
function setExt(name) {
  const parts = (name || "").split(".");
  const ext = (parts.length > 1 ? parts.pop() : "").toUpperCase().slice(0, 4);
  extEl.textContent = ext || "FILE";
  extEl.className = "ext named";
}

// --- base64url (no padding) → bytes -----------------------------------------
function b64urlToBytes(s) {
  s = s.replace(/-/g, "+").replace(/_/g, "/");
  while (s.length % 4) s += "=";
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// --- crypto, matching core/src/crypto.rs ------------------------------------
function nonceFor(index) {
  const n = new Uint8Array(12);
  new DataView(n.buffer).setUint32(0, index >>> 0, true); // LE index, rest zero
  return n;
}
function aadFor(index, total) {
  const a = new Uint8Array(8);
  const dv = new DataView(a.buffer);
  dv.setUint32(0, index >>> 0, true);
  dv.setUint32(4, total >>> 0, true);
  return a;
}
async function decChunk(key, index, total, ct) {
  const pt = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: nonceFor(index), additionalData: aadFor(index, total), tagLength: 128 },
    key,
    ct
  );
  return new Uint8Array(pt);
}

// --- exact-length reader over a fetch body stream ---------------------------
function byteReader(stream) {
  const reader = stream.getReader();
  let buf = new Uint8Array(0);
  let done = false;
  async function fill(n) {
    while (buf.length < n && !done) {
      const r = await reader.read();
      if (r.done) {
        done = true;
        break;
      }
      const nb = new Uint8Array(buf.length + r.value.length);
      nb.set(buf, 0);
      nb.set(r.value, buf.length);
      buf = nb;
    }
  }
  return {
    async exact(n) {
      await fill(n);
      if (buf.length < n) throw new Error(t("errTruncated"));
      const out = buf.slice(0, n);
      buf = buf.slice(n);
      return out;
    },
  };
}
const u32 = (b) => new DataView(b.buffer, b.byteOffset, 4).getUint32(0, true);
const u64 = (b) => Number(new DataView(b.buffer, b.byteOffset, 8).getBigUint64(0, true));

// --- capability detection ---------------------------------------------------
// Safari/WebKit ship service workers but their streaming-download support is
// unreliable, so we don't count it as a streaming target.
const ua = navigator.userAgent || "";
const isAppleWebKit = /AppleWebKit/.test(ua) && !/Chrome|Chromium|Edg|OPR/.test(ua);
const hasFsAccess = typeof window.showSaveFilePicker === "function";
const hasSwStreaming =
  "serviceWorker" in navigator && window.isSecureContext && !isAppleWebKit;
// Whether we can stream to disk without buffering the whole file in memory.
const canStream = hasFsAccess || hasSwStreaming;

// --- download sinks ---------------------------------------------------------
// 1) File System Access API (Chromium): true streaming to disk.
async function fsAccessSink(name) {
  const handle = await window.showSaveFilePicker({ suggestedName: name });
  const writable = await handle.createWritable();
  return {
    streaming: true,
    write: (bytes) => writable.write(bytes),
    close: () => writable.close(),
    abort: () => writable.abort().catch(() => {}),
  };
}

// 2) Service Worker stream (Firefox/Edge/Chrome without FS Access): pipes bytes
// to a real download with back-pressure (one chunk per pull), so memory stays
// bounded. Falls back if the handshake doesn't complete quickly.
async function swSink(name) {
  await navigator.serviceWorker.register("/arvolo-sw.js");
  await navigator.serviceWorker.ready;
  if (!navigator.serviceWorker.controller) {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("sw did not take control")), 3000);
      navigator.serviceWorker.addEventListener(
        "controllerchange",
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true }
      );
    });
  }
  const id = Math.random().toString(36).slice(2) + Date.now().toString(36);
  const mc = new MessageChannel();
  const port = mc.port1;

  const pullWaiters = [];
  const pullBacklog = [];
  const waitPull = () =>
    pullBacklog.length
      ? (pullBacklog.pop(), Promise.resolve())
      : new Promise((res) => pullWaiters.push(res));

  await new Promise((resolve, reject) => {
    // `timer`, not `t`: `t` is the translation lookup, and shadowing it inside a
    // function that reports failures is how a page ends up calling a timeout id.
    const timer = setTimeout(() => reject(new Error("sw handshake timed out")), 3000);
    port.onmessage = (e) => {
      const m = e.data || {};
      if (m.type === "ready") {
        clearTimeout(timer);
        resolve();
      } else if (m.type === "pull") {
        if (pullWaiters.length) pullWaiters.shift()();
        else pullBacklog.push(true);
      }
    };
    navigator.serviceWorker.controller.postMessage({ type: "register", id, name }, [mc.port2]);
  });

  // Kick off the actual browser download; the SW answers it with our stream.
  const iframe = document.createElement("iframe");
  iframe.hidden = true;
  iframe.src = "/dl/stream/" + id;
  document.body.appendChild(iframe);

  return {
    streaming: true,
    async write(bytes) {
      await waitPull(); // back-pressure: only send when the download pulls
      port.postMessage({ type: "chunk", bytes }, [bytes.buffer]);
    },
    async close() {
      await waitPull();
      port.postMessage({ type: "end" });
    },
    abort() {
      try {
        port.postMessage({ type: "abort" });
      } catch (_) {}
    },
  };
}

// 3) Blob fallback (Safari/older): buffers the plaintext in memory, then saves.
function blobSink(name) {
  const parts = [];
  return {
    streaming: false,
    write: (bytes) => {
      parts.push(bytes);
      return Promise.resolve();
    },
    close: () => {
      const blob = new Blob(parts, { type: "application/octet-stream" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = name;
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 60000);
      return Promise.resolve();
    },
    abort: () => {},
  };
}

async function makeSink(name) {
  if (hasFsAccess) {
    try {
      return await fsAccessSink(name);
    } catch (e) {
      if (e && e.name === "AbortError") throw e; // user cancelled the save dialog
      // else fall through
    }
  }
  if (hasSwStreaming) {
    try {
      return await swSink(name);
    } catch (e) {
      // streaming setup failed → buffered fallback
    }
  }
  showWarn(t("compat"));
  return blobSink(name);
}

// --- main flow --------------------------------------------------------------
const claim = location.pathname.split("/").filter(Boolean).pop() || "";
const fragment = location.hash.slice(1);
let key = null;

async function init() {
  if (!window.crypto || !crypto.subtle) {
    return fail(t("errNoCrypto"));
  }
  if (!fragment) return fail(t("errNoKey"));
  try {
    key = await crypto.subtle.importKey("raw", b64urlToBytes(fragment), "AES-GCM", false, ["decrypt"]);
  } catch (e) {
    return fail(t("errBadKey"));
  }
  setGo(t("download"), true);
  setStatus(t("ready"));
  if (!canStream) showWarn(t("compat"));
}

async function run() {
  setGo(t("downloading"), false, true);
  barEl.style.display = "block";
  fillEl.className = "";
  let sink = null;
  try {
    setStatus(t("fetching"));
    const resp = await fetch("/v1/fetch/" + encodeURIComponent(claim));
    if (!resp.ok || !resp.body) {
      throw new Error(t("errGone"));
    }
    const r = byteReader(resp.body);

    const magic = new TextDecoder().decode(await r.exact(8));
    if (magic !== MAGIC) throw new Error(t("errNotArvolo"));
    const chunkSize = u32(await r.exact(4));
    const totalSize = u64(await r.exact(8));
    const metaLen = u32(await r.exact(4));
    const totalChunks = chunkSize ? Math.ceil(totalSize / chunkSize) : 0;

    const metaCt = await r.exact(metaLen);
    let name;
    try {
      name = new TextDecoder().decode(await decChunk(key, META_INDEX, totalChunks, metaCt));
    } catch (e) {
      throw new Error(t("errDecrypt"));
    }
    nameEl.textContent = name;
    sizeEl.textContent = fmtSize(totalSize);
    setExt(name);

    sink = await makeSink(name);
    setStatus(t(sink.streaming ? "toDisk" : "toMemory"));

    let written = 0;
    for (let i = 0; i < totalChunks; i++) {
      const ctLen = u32(await r.exact(4));
      const ct = await r.exact(ctLen);
      const pt = await decChunk(key, i, totalChunks, ct);
      await sink.write(pt);
      written += pt.length;
      const pct = totalSize ? Math.floor((written / totalSize) * 100) : 100;
      fillEl.style.width = pct + "%";
    }
    await sink.close();
    fillEl.style.width = "100%";
    fillEl.className = "done";
    setGo(t("downloaded"), false);
    setStatus(t("done", fmtSize(totalSize)), "ok");
  } catch (e) {
    if (sink && sink.abort) sink.abort();
    if (e && e.name === "AbortError") {
      setStatus(t("cancelled"), "");
      setGo(t("download"), true);
      return;
    }
    fail((e && e.message) || t("errGeneric"));
  }
}

function fail(msg) {
  setStatus(msg, "err");
  fillEl.className = "bad";
  setGo(t("unavailable"), false);
}

applyI18n();
goEl.addEventListener("click", run);
init();

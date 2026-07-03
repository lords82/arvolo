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

const $ = (id) => document.getElementById(id);
const nameEl = $("name"),
  sizeEl = $("size"),
  goEl = $("go"),
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

function fmtSize(n) {
  const u = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return (i === 0 ? n : n.toFixed(1)) + " " + u[i];
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
      if (buf.length < n) throw new Error("truncated download");
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

const COMPAT_NOTE =
  "<b>This browser isn't fully compatible.</b> It can't stream the download to disk, " +
  "so the file is decrypted in memory — very large files may fail. " +
  "For big files, use Chrome, Edge, or Firefox.";

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
      const t = setTimeout(() => reject(new Error("sw did not take control")), 3000);
      navigator.serviceWorker.addEventListener(
        "controllerchange",
        () => {
          clearTimeout(t);
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
    const t = setTimeout(() => reject(new Error("sw handshake timed out")), 3000);
    port.onmessage = (e) => {
      const m = e.data || {};
      if (m.type === "ready") {
        clearTimeout(t);
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
  showWarn(COMPAT_NOTE);
  return blobSink(name);
}

// --- main flow --------------------------------------------------------------
const claim = location.pathname.split("/").filter(Boolean).pop() || "";
const fragment = location.hash.slice(1);
let key = null;

async function init() {
  if (!window.crypto || !crypto.subtle) {
    return fail("This browser can't decrypt here (WebCrypto unavailable — needs HTTPS).");
  }
  if (!fragment) return fail("This link is missing its decryption key (the part after #).");
  try {
    key = await crypto.subtle.importKey("raw", b64urlToBytes(fragment), "AES-GCM", false, ["decrypt"]);
  } catch (e) {
    return fail("This link's key is malformed.");
  }
  goEl.disabled = false;
  goEl.textContent = "Download";
  setStatus("Ready. Nothing is downloaded until you click.");
  if (!canStream) showWarn(COMPAT_NOTE);
}

async function run() {
  goEl.disabled = true;
  barEl.style.display = "block";
  let sink = null;
  try {
    setStatus("Fetching encrypted data…");
    const resp = await fetch("/v1/fetch/" + encodeURIComponent(claim));
    if (!resp.ok || !resp.body) {
      throw new Error("This link has expired or was already downloaded (one-time links burn on first pickup).");
    }
    const r = byteReader(resp.body);

    const magic = new TextDecoder().decode(await r.exact(8));
    if (magic !== MAGIC) throw new Error("This isn't an arvolo download link.");
    const chunkSize = u32(await r.exact(4));
    const totalSize = u64(await r.exact(8));
    const metaLen = u32(await r.exact(4));
    const totalChunks = chunkSize ? Math.ceil(totalSize / chunkSize) : 0;

    const metaCt = await r.exact(metaLen);
    let name;
    try {
      name = new TextDecoder().decode(await decChunk(key, META_INDEX, totalChunks, metaCt));
    } catch (e) {
      throw new Error("Decryption failed — wrong key or corrupted data.");
    }
    nameEl.textContent = name;
    sizeEl.textContent = fmtSize(totalSize);

    sink = await makeSink(name);
    setStatus(sink.streaming ? "Decrypting and saving to disk…" : "Decrypting in memory…");

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
    setStatus("Done — " + fmtSize(totalSize) + " decrypted and saved.", "ok");
  } catch (e) {
    if (sink && sink.abort) sink.abort();
    if (e && e.name === "AbortError") {
      setStatus("Save cancelled.", "");
      goEl.disabled = false;
      return;
    }
    fail((e && e.message) || "Download failed.");
  }
}

function fail(msg) {
  setStatus(msg, "err");
  goEl.disabled = true;
  goEl.textContent = "Unavailable";
}

goEl.addEventListener("click", run);
init();

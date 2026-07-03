// arvolo download service worker.
//
// Turns a page-driven byte stream into a real "save to disk" download that the
// browser writes incrementally, so a large file never has to sit in memory
// (the StreamSaver technique, self-hosted and same-origin).
//
// Protocol (all over a MessageChannel port the page hands us):
//   page → sw : {type:'register', id, name}  (+ the port)
//   sw   → page: {type:'ready'}
//   the page then navigates a hidden iframe to /dl/stream/<id>; we answer that
//   request with a streaming Response whose body we feed on demand:
//   sw   → page: {type:'pull'}                (back-pressure: send one more)
//   page → sw : {type:'chunk', bytes} | {type:'end'} | {type:'abort'}
//
// No Content-Length is sent, so the download ends exactly when the page says
// 'end' — which lets us apply real back-pressure (one chunk per pull) instead
// of buffering ahead.

"use strict";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

const streams = new Map(); // id -> { stream, name }

self.addEventListener("message", (e) => {
  const d = e.data || {};
  if (d.type !== "register") return;
  const port = e.ports[0];
  let deliver = null; // resolver for the in-flight pull

  const stream = new ReadableStream({
    pull(controller) {
      return new Promise((resolve) => {
        deliver = (msg) => {
          if (msg.type === "chunk") {
            controller.enqueue(new Uint8Array(msg.bytes));
            resolve();
          } else if (msg.type === "end") {
            controller.close();
            streams.delete(d.id);
            resolve();
          } else {
            controller.error(new Error("aborted"));
            streams.delete(d.id);
            resolve();
          }
        };
        port.postMessage({ type: "pull" });
      });
    },
    cancel() {
      try {
        port.postMessage({ type: "cancel" });
      } catch (_) {}
      streams.delete(d.id);
    },
  });

  port.onmessage = (ev) => {
    if (deliver) {
      const f = deliver;
      deliver = null;
      f(ev.data || {});
    }
  };

  streams.set(d.id, { stream, name: d.name });
  port.postMessage({ type: "ready" });
});

self.addEventListener("fetch", (e) => {
  const m = new URL(e.request.url).pathname.match(/\/dl\/stream\/([^/]+)$/);
  if (!m) return;
  const entry = streams.get(m[1]);
  if (!entry) return; // unknown id → normal 404
  const headers = new Headers({
    "Content-Type": "application/octet-stream",
    "Content-Disposition":
      "attachment; filename*=UTF-8''" + encodeURIComponent(entry.name),
    "Cache-Control": "no-store",
  });
  e.respondWith(new Response(entry.stream, { headers }));
});

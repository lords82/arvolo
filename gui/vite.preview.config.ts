import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/** Dev-only: the phone screens in a browser, with the tests' fake bridge. */
export default defineConfig({
  plugins: [
    react(),
    {
      name: "preview-stubs",
      enforce: "pre",
      async resolveId(source, importer) {
        if (source === "vitest") return "\0vitest";
        if (source.startsWith("@tauri-apps/")) return "\0tauri";
        if (/(^|\/)\.\.?\/ipc$/.test(source) && importer && !importer.includes("preview-ipc"))
          return (await this.resolve("/src/preview-ipc.ts"))?.id ?? null;
        return null;
      },
      load(id) {
        if (id === "\0vitest") return "export const vi = { fn: (f) => f ?? (() => {}) };";
        if (id === "\0tauri")
          return `
            export const invoke = async () => undefined;
            export const listen = async () => () => {};
            export const getCurrentWebview = () => ({ onDragDropEvent: async () => () => {} });
            export const getCurrentWindow = () => ({ setTitle: async () => {} });
            export const open = async () => null;
            export const save = async () => null;
            export const openUrl = async () => {};
            export const revealItemInDir = async () => {};
            export const isEnabled = async () => false;
            export const enable = async () => {};
            export const disable = async () => {};
            export const impactFeedback = async () => {};
            export const notificationFeedback = async () => {};
            export const selectionFeedback = async () => {};
            export const isPermissionGranted = async () => true;
            export const requestPermission = async () => "granted";
            export const sendNotification = () => {};
            // The scanner, as the plugin actually answers: a permission is a
            // bare string, and a windowed scan is a promise that stays pending
            // until a square is read. A preview has no camera behind the page,
            // so it stays pending for ever — which is what *looking at* the
            // viewfinder is.
            export const scan = () => new Promise(() => {});
            export const checkPermissions = async () => "granted";
            export const requestPermissions = async () => "granted";
            export const cancel = async () => {};
            export const Format = { QRCode: "QRCode" };
          `;
        return null;
      },
    },
  ],
  server: { port: 5199, open: false },
});

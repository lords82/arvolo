// Reading a code off another device's screen.
//
// Every code this app shows is also drawn as a QR, and a QR exists to be looked
// at by a camera. Until this was shared, only one of the two screens that ask
// for a code could do the looking: swapping contacts — the moment two people are
// most likely to be standing in front of each other with a square on one screen
// — offered a text field and nothing else, so the code had to be read out loud
// and typed, character by character, off a phone held up in the air.
//
// **The first tap is a different action from the second**, and that is the whole
// of what was wrong here. The first one has to get past the camera permission;
// only the ones after it are a scan. This asked for the scan directly and
// swallowed whatever came back, so the first tap did nothing at all — no
// viewfinder, no alert, no message — and the button worked from the second tap
// on, which is exactly what "it seems stuck" looks like from the outside.
//
// So the permission is asked for **here**, by name, before anything is scanned,
// and a refusal is said out loud with the way to undo it. Silence is not a
// state this button is allowed to be in: it is the state it was broken in.
//
// Loaded only when asked for: the plugin exists on phones and nowhere else, and
// a desktop bundle should not carry a camera it has no way to open.

import { t } from "./i18n";
import { toast } from "./ui/Toasts";

type Plugin = typeof import("@tauri-apps/plugin-barcode-scanner");

/** The plugin, or nothing. It exists on phones and nowhere else, and a desktop
 *  bundle should not carry a camera it has no way to open. */
async function plugin(): Promise<Plugin | null> {
  try {
    return await import("@tauri-apps/plugin-barcode-scanner");
  } catch {
    return null;
  }
}

/** Is there a camera this build can open at all? Asked by the screen that *is* a
 *  camera, so it can say what it is instead of showing a dead viewfinder. */
export async function hasCamera(): Promise<boolean> {
  return (await plugin()) !== null;
}

/** Asked for, not assumed. `checkPermissions` answers without prompting, so the
 *  prompt only ever appears the first time — which is the platform's own rule
 *  about when it is reasonable to ask. */
async function permission(api: Plugin): Promise<string> {
  const state = await api.checkPermissions();
  return state === "granted" ? state : await api.requestPermissions();
}

/** What a camera that was asked to look at something came back with.
 *
 *  A result rather than a string-or-null, because the screen in `ExchangeView`
 *  *is* the viewfinder: it has to draw the refusal and the missing plugin, and a
 *  toast fired every time that screen is opened would be the same sentence over
 *  and over for a state that is not going to change by itself. The one-shot
 *  button below keeps the toast — there the person just tapped, and a tap that
 *  says nothing is the thing that reads as broken. */
export type ScanResult =
  | { kind: "read"; content: string }
  /** Dismissed, or nothing in frame. Not a failure. */
  | { kind: "none" }
  | { kind: "denied" }
  | { kind: "absent" }
  | { kind: "failed"; why: string };

/** Open the camera *behind the webview* and wait for one square.
 *
 *  `windowed` is the whole reason this exists next to `scanOne`: the plugin can
 *  either take the screen over with its own viewfinder, or put the preview
 *  behind a transparent webview and leave the drawing to us. The design of
 *  record ("Arvolo Mobile 2 — Scambia", 4a) wants the second one — the camera is
 *  the screen, and the app is what is laid over it. */
export async function scanLive(): Promise<ScanResult> {
  const api = await plugin();
  if (!api) return { kind: "absent" };
  try {
    if ((await permission(api)) !== "granted") return { kind: "denied" };
  } catch (e) {
    return { kind: "failed", why: String(e) };
  }
  try {
    const res = await api.scan({ windowed: true, formats: [api.Format.QRCode] });
    const content = res?.content?.trim();
    return content ? { kind: "read", content } : { kind: "none" };
  } catch {
    // Cancelled — by `stopScan`, by leaving the screen, or by the system.
    return { kind: "none" };
  }
}

/** Shut the camera. Idempotent, and safe where there is no plugin: leaving the
 *  screen with a preview still running is a green light on a phone in a pocket. */
export async function stopScan(): Promise<void> {
  const api = await plugin();
  if (!api) return;
  await api.cancel().catch(() => {});
}

/** Take the person to the system's own settings, where a permanent refusal is
 *  the only place it can be undone. */
export async function openCameraSettings(): Promise<void> {
  const api = await plugin();
  if (!api) return;
  await api.openAppSettings().catch(() => {});
}

/** Scan one QR and return its contents, or `null` if nothing was read. */
export async function scanOne(): Promise<string | null> {
  const api = await plugin();
  if (!api) {
    // No plugin: a desktop, or a build without it. The field beside the button
    // still takes a typed code.
    return null;
  }

  let state: string;
  try {
    state = await permission(api);
  } catch (e) {
    toast.bad(t("scan.failed"), String(e));
    return null;
  }

  if (state !== "granted") {
    // "Nothing happened" was the old answer to this, and it is the one answer a
    // person cannot act on. Where the refusal is permanent, the only way back is
    // the system's own settings, so the message says so and the app offers to
    // open them.
    toast.bad(t("scan.noPermission"), t("scan.noPermissionHow"), {
      label: t("scan.openSettings"),
      run: () => void api.openAppSettings().catch(() => {}),
    });
    return null;
  }

  try {
    const res = await api.scan({ windowed: false, formats: [api.Format.QRCode] });
    return res?.content?.trim() || null;
  } catch {
    // Dismissed, or nothing read. Not worth a banner: the person is looking at
    // the field they can type into.
    return null;
  }
}

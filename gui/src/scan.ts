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

/** Scan one QR and return its contents, or `null` if nothing was read. */
export async function scanOne(): Promise<string | null> {
  let api: typeof import("@tauri-apps/plugin-barcode-scanner");
  try {
    api = await import("@tauri-apps/plugin-barcode-scanner");
  } catch {
    // No plugin: a desktop, or a build without it. The field beside the button
    // still takes a typed code.
    return null;
  }

  // Asked for, not assumed. `checkPermissions` answers without prompting, so
  // the prompt only ever appears the first time — which is the platform's own
  // rule about when it is reasonable to ask.
  let state: string;
  try {
    state = await api.checkPermissions();
    if (state !== "granted") {
      state = await api.requestPermissions();
    }
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

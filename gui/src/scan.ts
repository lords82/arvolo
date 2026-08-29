// Reading a code off another device's screen.
//
// Every code this app shows is also drawn as a QR, and a QR exists to be looked
// at by a camera. Until this was shared, only one of the two screens that ask
// for a code could do the looking: swapping contacts — the moment two people are
// most likely to be standing in front of each other with a square on one screen
// — offered a text field and nothing else, so the code had to be read out loud
// and typed, character by character, off a phone held up in the air.
//
// Loaded only when asked for: the plugin exists on phones and nowhere else, and
// a desktop bundle should not carry a camera it has no way to open. A refusal —
// no permission, no camera, the sheet dismissed — is not an error worth a red
// banner, so it comes back as `null` and the field beside it is still right
// there.

/** Scan one QR and return its contents, or `null` if nothing was read. */
export async function scanOne(): Promise<string | null> {
  try {
    const { scan, Format } = await import("@tauri-apps/plugin-barcode-scanner");
    const res = await scan({ windowed: false, formats: [Format.QRCode] });
    return res?.content?.trim() || null;
  } catch {
    return null;
  }
}

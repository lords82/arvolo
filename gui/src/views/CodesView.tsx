// Codes, both directions: the one you are showing and the one you are reading.
//
// On a desktop a code is a line of a send's row — you made it while sending, and
// you read it out. A phone is the device that is *held up*, and the one held in
// front of it: the two halves of a code exchange are two screens here, and a
// segmented control is the honest way to say that there are exactly two.
//
// Showing is not something this screen starts. A code is a way of reaching one
// particular send, so it is made where the send is made; what this shows is the
// one that is live, and what it says about it is the thing the desktop never has
// to: that it lasts only while the app is in front of you.

import { useState } from "react";
import { useT } from "../i18n";
import { useStore } from "../store";
import { CodeHero } from "../ui/Bits";
import { Icon } from "../ui/Icons";
import { Button, Empty, Segmented } from "../ui/Primitives";
import { toast } from "../ui/Toasts";

/** Read one code off another device's screen.
 *
 * Loaded only when asked for: the plugin exists on phones and nowhere else, and
 * a desktop bundle should not carry a camera it has no way to open. A refusal —
 * no permission, no camera, the sheet dismissed — is not an error worth a red
 * banner, so it comes back as `null` and the paste field is still right there. */
async function scanOne(): Promise<string | null> {
  try {
    const { scan, Format } = await import("@tauri-apps/plugin-barcode-scanner");
    const res = await scan({ windowed: false, formats: [Format.QRCode] });
    return res?.content?.trim() || null;
  } catch {
    return null;
  }
}

export function CodesView() {
  const t = useT();
  const [mode, setMode] = useState<"show" | "scan">("show");
  const transfers = useStore((s) => s.transfers);
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const openReceiveWith = useStore((s) => s.openReceiveWith);

  // Every send that is answering to a code right now. Usually one; more than one
  // is legitimate and the list says so rather than picking a winner.
  const hosted = Object.values(transfers).filter((x) => x.code);

  return (
    <div className="view-narrow codes">
      <Segmented
        block
        label={t("codes.mode")}
        value={mode}
        onChange={(v) => setMode(v as "show" | "scan")}
        options={[
          { value: "show", label: t("codes.show") },
          { value: "scan", label: t("codes.scan") },
        ]}
      />

      {mode === "show" ? (
        hosted.length ? (
          <>
            {hosted.map((tx) => (
              <CodeHero
                key={tx.key}
                value={tx.code!}
                caption={t("codes.whileOpen")}
              />
            ))}
            <div className="card card-pad t-xs t-mut">{t("codes.pauseNote")}</div>
          </>
        ) : (
          <div className="card">
            <Empty
              icon={<Icon.Qr size={22} />}
              title={t("codes.noneTitle")}
              action={
                <Button
                  variant="primary"
                  onClick={() => openSheet([], undefined, "code")}
                >
                  <Icon.Send size={14} />
                  {t("codes.hostAction")}
                </Button>
              }
            >
              {t("codes.noneBody")}
            </Empty>
          </div>
        )
      ) : (
        <div className="card card-pad stack">
          <div className="t-sm t-mut">{t("codes.scanHint")}</div>
          <Button
            variant="primary"
            block
            onClick={async () => {
              const got = await scanOne();
              if (got) openReceiveWith(got);
              else toast.info(t("codes.scanNothing"));
            }}
          >
            <Icon.Qr size={14} />
            {t("codes.scanAction")}
          </Button>
          {/* The other half of "read a code": the one somebody sent you in a
              message, which never went near a camera. */}
          <Button block onClick={openReceive}>
            {t("codes.pasteAction")}
          </Button>
        </div>
      )}
    </div>
  );
}

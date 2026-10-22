// La camera, dietro la scheda «Ce l'ho qui davanti» di Scambia.
//
// Un mirino senza cornice intorno: l'anteprima è dietro la webview (vedi
// `scan.ts` e `html.cam`), e qui c'è solo quello che l'app ha da dire —
// quattro angoli e una riga. Un quadrato letto fa partire un trasferimento, e il
// posto che mostra i trasferimenti è Attività.
//
// Due cose del design non ci sono, e sono rifiuti della piattaforma: niente
// torcia (il plugin non la espone), e un quadrato letto qui è sempre tentato come
// *ricezione* — il codice di uno scambio contatti e un codice d'invio hanno la
// stessa forma sul filo. «Inquadra e partono» (11e) aspetta l'engine.

import { useEffect, useRef, useState } from "react";
import { useStore } from "../store";
import { useT } from "../i18n";
import { openCameraSettings, scanLive, stopScan } from "../scan";
import { Icon } from "../ui/Icons";
import { Button } from "../ui/Primitives";
import { toast } from "../ui/Toasts";
import { Frame } from "./ExchangeView";

export function CameraView() {
  const t = useT();
  const go = useStore((s) => s.go);
  const receive = useStore((s) => s.receive);
  const [cam, setCam] = useState<"live" | "none" | "denied" | "absent" | "failed">(
    "live"
  );
  /** Bumped to ask for the camera again after it came back with nothing. */
  const [attempt, setAttempt] = useState(0);

  // One square, then this screen is done with the camera.
  //
  // `live` is optimistic — the plugin tells us when a scan *ends*, never when the
  // preview begins — so the only thing that corrects it is a refusal, and that
  // arrives in milliseconds on a build with no camera in it.
  const handle = useRef<(s: string) => void>(() => {});
  handle.current = async (content: string) => {
    try {
      await receive(content, null, null);
      toast.echo(t("receive.started"), t("receive.startedDetail"));
      go("transfers");
    } catch {
      // `act` already said what the daemon refused; stay here so the viewfinder
      // is still under the thumb.
      setAttempt((n) => n + 1);
    }
  };

  // Something laid over the screen — an offer, a person — means nobody is
  // looking through the camera. Leaving the preview running behind it is a
  // battery cost and a recording light on a phone whose owner is reading.
  const covered = useStore(
    (st) =>
      st.sheetPicks !== null ||
      st.incomingOfferId !== null ||
      st.receiveOpen ||
      st.personOpen !== null ||
      st.pairing !== null
  );

  useEffect(() => {
    if (covered) {
      void stopScan();
      return;
    }
    let alive = true;
    setCam("live");
    document.documentElement.classList.add("cam");
    void scanLive().then((r) => {
      if (!alive) return;
      if (r.kind === "read") {
        setCam("none");
        handle.current(r.content);
        return;
      }
      if (r.kind === "failed") toast.bad(t("scan.failed"), r.why);
      setCam(r.kind === "failed" ? "failed" : r.kind);
    });
    return () => {
      alive = false;
      document.documentElement.classList.remove("cam");
      void stopScan();
    };
    // `t` is read inside the callback only for a failure message; re-running this
    // on a language change would restart the camera.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attempt, covered]);

  return (
    <div className="exch">
      {/* The way back floats over the preview: this screen has no bar. */}
      <button className="cam-back" aria-label={t("common.back")} onClick={() => window.history.back()}>
        <Icon.ChevronLeft size={22} />
      </button>
      <div className={`exch-cam${cam === "live" ? " live" : ""}`}>
        <Frame size={216} />
        {cam === "live" ? (
          <div className="exch-say">{t("exchange.aim")}</div>
        ) : cam === "denied" ? (
          <div className="exch-say">
            {t("scan.noPermission")}
            <span className="exch-say-sub">{t("scan.noPermissionHow")}</span>
            <Button size="sm" onClick={() => void openCameraSettings()}>
              {t("scan.openSettings")}
            </Button>
          </div>
        ) : cam === "absent" ? (
          <div className="exch-say">{t("exchange.noCamera")}</div>
        ) : (
          <div className="exch-say">
            {t("exchange.again")}
            <Button size="sm" onClick={() => setAttempt((n) => n + 1)}>
              <Icon.Scan size={14} /> {t("exchange.aimAgain")}
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

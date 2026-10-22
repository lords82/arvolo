// Il tuo indirizzo, intero.
//
// In *Avanzate* era una riga di lista, e una riga di lista non può contenerlo:
// l'id è base32 lungo una cinquantina di caratteri più il relay, quindi la riga
// ne mostrava i primi dieci e il resto era nel tocco — che copia, senza mai far
// vedere cosa. Una cosa che si dà a qualcuno va letta prima di darla.
//
// Qui invece c'è per intero, va a capo, si può selezionare a mano, e il tasto
// sotto lo mette negli appunti.
//
// **Niente QR.** Un quadrato ha senso dove qualcuno può inquadrarlo, e la camera
// di Arvolo (`CameraView`) passa quello che legge a `receive()`: legge codici di
// invio, non indirizzi. Disegnarlo lo farebbe sembrare scansionabile e non lo
// sarebbe — il giorno in cui la camera riconoscerà un indirizzo, il quadrato
// viene qui e la riga sotto se ne va.

import { useStore } from "../store";
import { useT } from "../i18n";
import { haptic } from "../haptics";
import { Icon } from "../ui/Icons";
import { useCopy } from "../ui/Bits";

export function IdentityView() {
  const t = useT();
  const status = useStore((s) => s.status);
  const { state, copy } = useCopy();
  // L'indirizzo da dare è id *e* relay: l'id da solo dice chi sei e non dove
  // ascolti. Il nudo resta per un daemon troppo vecchio per comporre la coppia.
  const value = status?.shareable_id || status?.public_id || "";

  return (
    <div className="ios-page">
      <div className="ios-group">
        <div className="id-full mono">{value || "…"}</div>
        <button
          className="ios-row action"
          disabled={!value}
          onClick={() => {
            void copy(value).then(() => haptic.did());
          }}
        >
          {state === "ok" ? <Icon.Check size={15} /> : <Icon.Copy size={15} />}
          {state === "ok"
            ? t("common.copied")
            : state === "fail"
              ? t("common.copyFailed")
              : t("common.copy")}
        </button>
      </div>
      <div className="ios-foot">{t("identity.what")}</div>
      <div className="ios-foot">{t("identity.noQr")}</div>
    </div>
  );
}

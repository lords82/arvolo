// Everything else, on a phone.
//
// The desktop's settings are a window: eight cards, each with a title, captioned
// fields and a Save beside every one of them. That shape was the last thing in
// the app still drawn for a mouse — pushed into from *You*, it arrived as a page
// of forms in the middle of a phone made of lists.
//
// So: grouped inset lists, a value on the right of its own row, saving on blur,
// and the explanation under the group rather than under the field.
//
// What is *not* here is the other half of the redesign. Four of the desktop's
// controls have nothing to act on here — a phone has no login items, no
// self-updating binary, no daemon of its own to restart (the engine is inside
// this process), and no text editor to open `config.toml` in. Showing them
// disabled would be the app claiming a capability and then refusing it; the
// honest version is a screen with only the rows that do something.
//
// And three more things left later, for the same kind of reason:
//
// - **Il file di configurazione.** Una riga che dava un percorso dentro la
//   sandbox del telefono, che da qui non si apre, non si modifica e non si
//   raggiunge. Sapere dove vive non serve a chi non può andarci.
// - **L'impronta.** È già in Impostazioni, intera, sotto il nome: ripeterla qui
//   accorciata significava mostrare *peggio* lo stesso valore.
// - **Versione e motore.** Non sono una regolazione avanzata; il numero di
//   build sta nel piede di Impostazioni, che è dove questa piattaforma lo
//   scrive. Sul telefono motore e app sono lo stesso binario, quindi erano due
//   righe che non potevano discordare.
//
// Resta l'indirizzo, che però non è una riga: `IdentityView`.

import { useT } from "../i18n";
import { fire, useStore } from "../store";
import { haptic } from "../haptics";
import { Empty } from "../ui/Primitives";
import { Place, ToggleRow } from "../ui/Rows";
import { Icon } from "../ui/Icons";
import { toast } from "../ui/Toasts";


export function AdvancedView() {
  const t = useT();
  const config = useStore((s) => s.config);
  const error = useStore((s) => s.configError);
  const save = useStore((s) => s.saveConfig);
  const push = useStore((s) => s.push);

  if (!config) {
    return (
      <div className="ios-page">
        <Empty
          icon={<Icon.Settings size={22} />}
          title={error ?? t("common.loading")}
        />
      </div>
    );
  }

  return (
    <div className="ios-page">
      {/* L'indirizzo non sta in una riga — ci sta la porta per andarlo a
          leggere. L'impronta invece non è qui affatto: è in Impostazioni,
          intera, ed è lì che si detta. */}
      <div className="ios-head">{t("adv.identity")}</div>
      <div className="ios-group">
        <Place label={t("identity.title")} onClick={() => push("identity")} />
      </div>

      {/* ---- files ------------------------------------------------------ */}
      {/* No "where files land" row. It named a folder that cannot be changed on
          either phone — `Download/Arvolo` through MediaStore, `Documents`
          through Files — and a screen of settings is a screen of things that
          are settable. What a person actually wants from it is the file, and
          that is one tap on the row that received it. */}
      <div className="ios-head">{t("settings.files")}</div>
      <div className="ios-group">
        <ToggleRow
          label={t("settings.seedTitle")}
          checked={config.seed ?? true}
          onChange={(v) =>
            fire(
              save({ seed: { set: v } }).then(() => {
                haptic.did();
                toast.info(t("settings.saved"), t("common.afterRestart"));
              })
            )
          }
        />
      </div>
      <div className="ios-foot">{t("settings.seedDesc")}</div>
    </div>
  );
}

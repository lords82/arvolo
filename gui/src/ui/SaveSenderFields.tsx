// Naming a stranger, in the one shape both places that ask should use.
//
// Two screens ask this question — the offer dialog, while a decision is open,
// and the board's "Save the sender…" for an arrival nobody was there to greet.
// They used to ask it with two different fields: one proposed a name from what
// the sender calls themselves, the other started blank, and only one of them
// said that the proposal was the sender's own text.
//
// The collision check is the part that matters. `contact_add` re-points a name
// that already exists at the new key and drops that name's verified and trusted
// marks — so typing "giulia" over a stranger's offer would quietly demote the
// Giulia you had checked. The daemon refuses it too (that is the guarantee); this
// is so the refusal arrives while you are still typing, and names the person you
// were about to overwrite.

import { useMemo } from "react";
import type { ContactDto } from "../types";
import { useT } from "../i18n";
import { Field, TextInput } from "./Primitives";

/** The contact this name already belongs to, when it is somebody else. */
export function nameCollision(
  contacts: ContactDto[],
  name: string,
  peerId: string | undefined
): ContactDto | undefined {
  const wanted = name.trim();
  if (!wanted) return undefined;
  return contacts.find((c) => c.name === wanted && c.id !== peerId);
}

export function SaveSenderFields({
  value,
  onChange,
  contacts,
  peerId,
  suggested,
  onSubmit,
  autoFocus,
}: {
  value: string;
  onChange: (v: string) => void;
  contacts: ContactDto[];
  peerId: string | undefined;
  /** Whether `value` is still the proposal nobody has edited — the hint says
   *  where it came from only while that is true, because after an edit the name
   *  is the user's and saying otherwise would be wrong. */
  suggested: boolean;
  onSubmit?: () => void;
  autoFocus?: boolean;
}) {
  const t = useT();
  const clash = useMemo(
    () => nameCollision(contacts, value, peerId),
    [contacts, value, peerId]
  );

  return (
    <Field
      label={t("incoming.saveAsLabel")}
      error={clash ? t("incoming.nameTaken", clash.name) : null}
      hint={suggested && value ? t("incoming.saveAsSuggested") : t("incoming.saveNote")}
    >
      {({ id, describedBy }) => (
        <TextInput
          id={id}
          aria-describedby={describedBy}
          aria-invalid={clash ? true : undefined}
          data-autofocus={autoFocus ? "" : undefined}
          value={value}
          onChange={(e) => onChange(e.currentTarget.value)}
          placeholder={t("incoming.saveAsPlaceholder")}
          onKeyDown={(e) => {
            if (e.key === "Enter" && onSubmit) {
              e.preventDefault();
              onSubmit();
            }
          }}
        />
      )}
    </Field>
  );
}

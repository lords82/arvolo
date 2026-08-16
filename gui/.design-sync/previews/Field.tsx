import { Field, TextInput, Textarea } from "arvolo-gui";

export const WithHint = () => (
  <div style={{ width: 340 }}>
    <Field label="Display name" hint="Shown to people you pair with.">
      {({ id, describedBy }) => (
        <TextInput
          id={id}
          aria-describedby={describedBy}
          defaultValue="Lorenzo's MacBook"
        />
      )}
    </Field>
  </div>
);

export const WithError = () => (
  <div style={{ width: 340 }}>
    <Field
      label="Relay address"
      error="That host didn't answer on port 8787."
      hint="Leave empty to use the default relay."
    >
      {({ id, describedBy }) => (
        <TextInput
          id={id}
          aria-describedby={describedBy}
          defaultValue="relay.example.invalid"
        />
      )}
    </Field>
  </div>
);

export const MultiLine = () => (
  <div style={{ width: 340 }}>
    <Field label="Note for the recipient" hint="Optional. 200 characters max.">
      {({ id, describedBy }) => (
        <Textarea
          id={id}
          aria-describedby={describedBy}
          rows={3}
          defaultValue="Contract draft — the signed copy follows tomorrow."
        />
      )}
    </Field>
  </div>
);

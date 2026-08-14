import { Sheet, Field, TextInput, Button, CopyField } from "arvolo-gui";

/** Sheet is `position: fixed` — the `transform` on this wrapper makes it a
 *  containing block, so the sheet and its scrim render inside the card instead
 *  of escaping to the viewport. In a real app there is no wrapper: the sheet
 *  covers the window. */
const Stage = ({ children }: { children: React.ReactNode }) => (
  <div
    style={{
      position: "relative",
      transform: "translateZ(0)",
      width: 720,
      height: 460,
      overflow: "hidden",
      borderRadius: 10,
      background: "var(--canvas)",
    }}
  >
    {children}
  </div>
);

/** Sheets are modal: they render their own scrim over the whole surface. */
export const SideSheet = () => (
  <Stage>
  <Sheet
    open
    onClose={() => {}}
    title="Send files"
    subtitle="To Lorenzo's MacBook — paired, verified"
    footer={
      <>
        <Button>Cancel</Button>
        <Button variant="primary">Send 3 files</Button>
      </>
    }
  >
    <Field label="Note for the recipient" hint="Optional.">
      {({ id, describedBy }) => (
        <TextInput
          id={id}
          aria-describedby={describedBy}
          defaultValue="Contract draft"
        />
      )}
    </Field>
    <div style={{ marginTop: 16 }}>
      <CopyField value="arvolo1q9f3ac1k7q2m9xb4t8wz6" />
    </div>
  </Sheet>
  </Stage>
);

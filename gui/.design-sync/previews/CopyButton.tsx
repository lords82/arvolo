import { CopyButton, CopyField } from "arvolo-gui";

export const Default = () => (
  <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
    <span style={{ fontFamily: "var(--mono)", fontSize: 13 }}>
      k7q2m9xb4t8w
    </span>
    <CopyButton value="k7q2m9xb4t8w" />
  </div>
);

export const WithCustomLabel = () => (
  <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
    <span style={{ fontFamily: "var(--mono)", fontSize: 13 }}>
      https://arvolo.app/d/9f3ac1
    </span>
    <CopyButton value="https://arvolo.app/d/9f3ac1" label="Copy link" />
  </div>
);

/** In practice it usually arrives already composed, inside CopyField. */
export const InsideCopyField = () => (
  <div style={{ width: 340 }}>
    <CopyField value="arvolo1q9f3ac1k7q2m9xb4t8wz6" />
  </div>
);

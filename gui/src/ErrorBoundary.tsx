import { Component, type ErrorInfo, type ReactNode } from "react";

/** A render that throws unmounts the whole tree and leaves an empty window — the
 *  worst failure this app has, because it looks like a freeze and says nothing, and
 *  a release build has no devtools to ask. Show what broke instead, and offer a way
 *  back that does not involve killing the process. */
export class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Also goes to the webview console, for a dev build.
    console.error("Arvolo UI crashed:", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div
        style={{
          height: "100%",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 14,
          padding: 32,
          textAlign: "center",
          background: "var(--canvas)",
        }}
      >
        <div style={{ fontSize: 32 }}>⚠</div>
        <div style={{ fontSize: 15, fontWeight: 700 }}>
          Qualcosa si è rotto nell'interfaccia
        </div>
        <div style={{ fontSize: 12.5, color: "#57534c", maxWidth: 460, lineHeight: 1.5 }}>
          I trasferimenti non si fermano: continuano nel daemon in background. Puoi
          riprendere da dove eri.
        </div>
        <pre
          className="mono selectable"
          style={{
            maxWidth: 520,
            maxHeight: 180,
            overflow: "auto",
            textAlign: "left",
            fontSize: 11,
            background: "#fdecec",
            color: "#b91c1c",
            padding: "10px 12px",
            borderRadius: 10,
            whiteSpace: "pre-wrap",
          }}
        >
          {error.message}
          {error.stack ? `\n\n${error.stack}` : ""}
        </pre>
        <button
          onClick={() => this.setState({ error: null })}
          style={{
            border: "none",
            background: "#171514",
            color: "#fff",
            borderRadius: 10,
            padding: "10px 18px",
            fontSize: 13,
            fontWeight: 600,
            cursor: "pointer",
          }}
        >
          Riprova
        </button>
      </div>
    );
  }
}

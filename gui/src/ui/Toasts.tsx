// Transient notices, stacked bottom-right.
//
// The rule this file exists to enforce: an *error* never disappears on a timer.
// Successes are courtesies and can fade; a refusal is information the user has
// to act on, and a message that removed itself before it was read is how the
// board came to look like it was ignoring clicks. Errors sit until dismissed.

import { useEffect } from "react";
import { create } from "zustand";
import { Icon } from "./Icons";
import { IconButton } from "./Primitives";

export type ToastKind = "ok" | "bad" | "info";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string;
  /** An optional single action, e.g. "Riprova" or "Apri cartella". */
  action?: { label: string; run: () => void };
}

interface ToastState {
  items: Toast[];
  push: (t: Omit<Toast, "id">) => number;
  dismiss: (id: number) => void;
}

let seq = 0;

export const useToasts = create<ToastState>((set) => ({
  items: [],
  push: (t) => {
    const id = ++seq;
    set((s) => ({ items: [...s.items, { ...t, id }].slice(-4) }));
    return id;
  },
  dismiss: (id) => set((s) => ({ items: s.items.filter((i) => i.id !== id) })),
}));

/** Convenience wrappers so call sites read as the thing they are reporting. */
export const toast = {
  ok: (title: string, detail?: string) =>
    useToasts.getState().push({ kind: "ok", title, detail }),
  info: (title: string, detail?: string) =>
    useToasts.getState().push({ kind: "info", title, detail }),
  bad: (title: string, detail?: string, action?: Toast["action"]) =>
    useToasts.getState().push({ kind: "bad", title, detail, action }),
};

const AUTO_DISMISS_MS = 4200;

function ToastRow({ t }: { t: Toast }) {
  const dismiss = useToasts((s) => s.dismiss);
  useEffect(() => {
    if (t.kind === "bad") return; // see the note at the top of this file
    const h = window.setTimeout(() => dismiss(t.id), AUTO_DISMISS_MS);
    return () => window.clearTimeout(h);
  }, [t.id, t.kind, dismiss]);

  return (
    <div
      className={`toast ${t.kind === "bad" ? "bad" : t.kind === "ok" ? "ok" : ""}`}
      role={t.kind === "bad" ? "alert" : "status"}
    >
      <span
        className={
          t.kind === "bad" ? "tone-bad" : t.kind === "ok" ? "tone-ok" : "tone-in"
        }
        style={{ marginTop: 1 }}
      >
        {t.kind === "bad" ? (
          <Icon.Alert />
        ) : t.kind === "ok" ? (
          <Icon.Check />
        ) : (
          <Icon.Info />
        )}
      </span>
      <div className="grow">
        <div style={{ fontWeight: 570 }}>{t.title}</div>
        {t.detail && (
          <div
            className="t-sm t-sec"
            style={{ marginTop: 2, wordBreak: "break-word" }}
          >
            {t.detail}
          </div>
        )}
        {t.action && (
          <button
            className="btn btn-sm"
            style={{ marginTop: 8 }}
            onClick={() => {
              dismiss(t.id);
              t.action!.run();
            }}
          >
            {t.action.label}
          </button>
        )}
      </div>
      <IconButton label="Chiudi" onClick={() => dismiss(t.id)}>
        <Icon.Close size={13} />
      </IconButton>
    </div>
  );
}

export function ToastHost() {
  const items = useToasts((s) => s.items);
  if (!items.length) return null;
  return (
    <div className="toasts" aria-live="polite">
      {items.map((t) => (
        <ToastRow key={t.id} t={t} />
      ))}
    </div>
  );
}

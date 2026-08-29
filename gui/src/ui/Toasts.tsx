// Transient notices, stacked bottom-right.
//
// The rule this file exists to enforce: an *error* never disappears on a timer.
// Successes are courtesies and can fade; a refusal is information the user has
// to act on, and a message that removed itself before it was read is how the
// board came to look like it was ignoring clicks. Errors sit until dismissed.

import { useEffect } from "react";
import { create } from "zustand";
import { useT } from "../i18n";
import { isPhoneNow } from "../phone";
import { Icon } from "./Icons";
import { IconButton } from "./Primitives";

export type ToastKind = "ok" | "bad" | "info";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string;
  /** An optional single action, e.g. "Retry" or "Open the folder". Already
   *  translated by the caller: a toast is raised, not rendered, from the store. */
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
  /// A confirmation of something the screen is already showing.
  ///
  /// "Transfer started" while the row appears with its progress bar; "renamed"
  /// while the name changes under your eyes. On a desktop that is a courtesy —
  /// the window is large and the change may be in a column you were not looking
  /// at. On a phone it is noise, and expensive noise: it covers the bottom of a
  /// screen that is all there is, over the very change it is announcing.
  ///
  /// This platform's answer to "it worked" is that you can see it worked, so
  /// here that is the answer too — and where the result is *not* visible (a file
  /// written elsewhere, a person blocked from a screen that does not show them)
  /// the call stays `ok`, because then there is something to say.
  echo: (title: string, detail?: string) => {
    if (isPhoneNow()) return;
    useToasts.getState().push({ kind: "ok", title, detail });
  },
  info: (title: string, detail?: string) =>
    useToasts.getState().push({ kind: "info", title, detail }),
  bad: (title: string, detail?: string, action?: Toast["action"]) =>
    useToasts.getState().push({ kind: "bad", title, detail, action }),
};

const AUTO_DISMISS_MS = 4200;

function ToastRow({ t: item }: { t: Toast }) {
  const t = useT();
  const dismiss = useToasts((s) => s.dismiss);
  useEffect(() => {
    if (item.kind === "bad") return; // see the note at the top of this file
    const h = window.setTimeout(() => dismiss(item.id), AUTO_DISMISS_MS);
    return () => window.clearTimeout(h);
  }, [item.id, item.kind, dismiss]);

  return (
    <div
      className={`toast ${item.kind === "bad" ? "bad" : item.kind === "ok" ? "ok" : ""}`}
      role={item.kind === "bad" ? "alert" : "status"}
    >
      <span
        className={
          item.kind === "bad"
            ? "tone-bad"
            : item.kind === "ok"
              ? "tone-ok"
              : "tone-in"
        }
        style={{ marginTop: 1 }}
      >
        {item.kind === "bad" ? (
          <Icon.Alert />
        ) : item.kind === "ok" ? (
          <Icon.Check />
        ) : (
          <Icon.Info />
        )}
      </span>
      <div className="grow">
        <div style={{ fontWeight: 570 }}>{item.title}</div>
        {/* Wrapping long unbreakable runs is `.toast .grow` in theme.css, which
            covers the title too — it takes file paths, and this one used to be
            the only half of the toast that could hold a ticket without spilling. */}
        {item.detail && (
          <div className="t-sm t-sec" style={{ marginTop: 2 }}>
            {item.detail}
          </div>
        )}
        {item.action && (
          <button
            className="btn btn-sm"
            style={{ marginTop: 8 }}
            onClick={() => {
              dismiss(item.id);
              item.action!.run();
            }}
          >
            {item.action.label}
          </button>
        )}
      </div>
      <IconButton label={t("common.close")} onClick={() => dismiss(item.id)}>
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

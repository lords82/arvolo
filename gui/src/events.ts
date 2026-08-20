// The seam between the daemon's wire format and the app's event model.
//
// `EventDto` in Rust is a plain serde enum, so it derives an **externally tagged**
// representation: a struct variant is `{ variant_name: { ...fields } }` and a unit
// variant is the bare string `"variant_name"`. It is NOT `{ type, ...fields }`.
//
// Getting this wrong is silent and total: `switch (ev.type)` on an externally
// tagged event matches nothing, every event is dropped, and the board only ever
// shows its opening snapshot — looking, from the outside, exactly like an app that
// "does not update". Keep the wire shape here, in one place, verified by tests
// against real captured daemon output (see events.test.ts and the Rust-side
// `event_wire_format_is_stable`).

import type { EngineEvent, PairKind } from "./types";

/** An engine event exactly as it arrives from the daemon. */
export type WireEvent =
  | {
      offer_received: {
        id: string;
        from: string;
        name: string;
        size: number;
        note: string;
        sender_name: string;
      };
    }
  | {
      started: {
        id: number;
        direction: "send" | "recv";
        name: string;
        total_size: number;
      };
    }
  | { progress: { id: number; transferred: number; total_size: number } }
  | { completed: { id: number; path: string | null } }
  | { deposited: { id: number } }
  | { waiting: { id: number; reason: string } }
  | { paused: { id: number; reason: string } }
  | { failed: { id: number; error: string } }
  | { cancelled: { id: number } }
  | { code_ready: { id: number; code: string } }
  | { code_paired: { id: number; done: number } }
  | { code_closed: { id: number; reason: string } }
  | { pairing_code: { session: string; code: string } }
  | {
      pairing_done: {
        session: string;
        kind: PairKind;
        summary: string;
        needs_restart: boolean;
      };
    }
  | {
      pairing_failed: {
        session: string;
        kind: PairKind;
        error: string;
        cancelled: boolean;
      };
    }
  | "contacts_changed"
  | { offer_gone: { id: string } }
  | "finished_cleared";

/** Every variant the app knows how to act on. A daemon of a different build may
 *  send something else; that is ignored rather than crashing the board. */
const KNOWN = new Set<EngineEvent["type"]>([
  "offer_received",
  "started",
  "progress",
  "completed",
  "deposited",
  "waiting",
  "paused",
  "failed",
  "cancelled",
  "code_ready",
  "code_paired",
  "code_closed",
  "contacts_changed",
  "offer_gone",
  "finished_cleared",
  "pairing_code",
  "pairing_done",
  "pairing_failed",
]);

/** Flatten a wire event into the `{ type, ...fields }` shape the store switches on.
 *  Returns `null` for anything unrecognised — an unknown event must not take the
 *  window down. */
export function normalizeEvent(wire: unknown): EngineEvent | null {
  // Unit variants (e.g. `contacts_changed`) arrive as a bare string.
  if (typeof wire === "string") {
    return KNOWN.has(wire as EngineEvent["type"])
      ? ({ type: wire } as EngineEvent)
      : null;
  }
  if (!wire || typeof wire !== "object") return null;

  const entries = Object.entries(wire as Record<string, unknown>);
  if (entries.length !== 1) return null;
  const [tag, fields] = entries[0];
  if (!KNOWN.has(tag as EngineEvent["type"])) return null;
  if (!fields || typeof fields !== "object") return null;

  return { type: tag, ...(fields as object) } as EngineEvent;
}

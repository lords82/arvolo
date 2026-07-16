// The wire contract, pinned against what the daemon really sends.
//
// This is the seam that broke silently: `EventDto` derives serde's **externally
// tagged** shape (`{ started: {...} }`), the store switched on `ev.type`, nothing
// ever matched, and every live update was dropped — the board looked frozen while
// the daemon worked fine. The TypeScript even carried a comment claiming the events
// were internally tagged. Nothing caught it because the test mock invented the same
// wrong shape.
//
// So these fixtures are not hand-written from the type: they are copied from a real
// capture off the daemon socket, and the Rust side asserts the same bytes in
// `event_wire_format_is_stable`. If serde's output ever changes, both fail.

import { describe, expect, it } from "vitest";
import { normalizeEvent } from "../events";

describe("wire → app model", () => {
  it("55. the exact line the daemon sent us flattens into a usable event", () => {
    // Captured verbatim from `{"event":{...}}` on ~/.config/arvolo/daemon.sock.
    const captured = JSON.parse(
      '{"started":{"id":3,"direction":"send","name":"evtest.txt","total_size":6}}'
    );
    expect(normalizeEvent(captured)).toEqual({
      type: "started",
      id: 3,
      direction: "send",
      name: "evtest.txt",
      total_size: 6,
    });
  });

  it("56. every struct variant keeps all of its fields", () => {
    const cases: [unknown, Record<string, unknown>][] = [
      [
        {
          offer_received: {
            id: "o1",
            from: "peer1",
            name: "f.zip",
            size: 9,
            note: "ciao",
            sender_name: "Marta",
          },
        },
        {
          type: "offer_received",
          id: "o1",
          from: "peer1",
          name: "f.zip",
          size: 9,
          note: "ciao",
          sender_name: "Marta",
        },
      ],
      [
        { progress: { id: 1, transferred: 5, total_size: 10 } },
        { type: "progress", id: 1, transferred: 5, total_size: 10 },
      ],
      [
        { completed: { id: 1, path: "/Users/ls/Arvolo/f" } },
        { type: "completed", id: 1, path: "/Users/ls/Arvolo/f" },
      ],
      [{ completed: { id: 1, path: null } }, { type: "completed", id: 1, path: null }],
      [{ deposited: { id: 2 } }, { type: "deposited", id: 2 }],
      [
        { waiting: { id: 3, reason: "relay unavailable" } },
        { type: "waiting", id: 3, reason: "relay unavailable" },
      ],
      [
        { paused: { id: 4, reason: "by user" } },
        { type: "paused", id: 4, reason: "by user" },
      ],
      [{ failed: { id: 5, error: "boom" } }, { type: "failed", id: 5, error: "boom" }],
      [{ cancelled: { id: 6 } }, { type: "cancelled", id: 6 }],
    ];
    for (const [wire, want] of cases) {
      expect(normalizeEvent(wire), JSON.stringify(wire)).toEqual(want);
    }
  });

  it("57. a unit variant arrives as a bare string, not an object", () => {
    // serde renders `ContactsChanged` as just `"contacts_changed"`. Treating every
    // event as an object would drop it.
    expect(normalizeEvent("contacts_changed")).toEqual({ type: "contacts_changed" });
  });

  it("58. an event this build does not know is ignored, not fatal", () => {
    // A daemon from a different build may send more than we understand. Dropping it
    // is fine; throwing inside the listener is not.
    expect(normalizeEvent({ some_future_event: { id: 1 } })).toBeNull();
    expect(normalizeEvent("some_future_unit_event")).toBeNull();
  });

  it("59. junk never reaches the store", () => {
    for (const junk of [null, undefined, 42, [], {}, { a: 1, b: 2 }, { started: 5 }]) {
      expect(normalizeEvent(junk), String(junk)).toBeNull();
    }
  });

  it("60. the already-flattened shape is NOT accepted", () => {
    // Guards the mistake this whole seam exists to prevent: if `{type:...}` were
    // waved through, a mock (or a future refactor) could quietly go back to testing
    // an invented contract instead of the daemon's.
    expect(normalizeEvent({ type: "started", id: 1 })).toBeNull();
  });
});

import { Progress } from "arvolo-gui";

const base = {
  key: "t1",
  id: 1,
  name: "contract-draft.pdf",
  size: 4_800_000,
  transferred: 3_100_000,
  encrypted: true,
  verified: true,
  method: "p2p",
  swarmPeers: 1,
  downloadPeers: 1,
  files: 1,
} as const;

const Row = ({ label, children }: { label: string; children: React.ReactNode }) => (
  <div style={{ width: 320, marginBottom: 14 }}>
    <div style={{ font: "var(--t-hint, 12px system-ui)", color: "var(--ink-3)", marginBottom: 6 }}>
      {label}
    </div>
    {children}
  </div>
);

/** Direction is a colour: outgoing is warm, incoming is cool. */
export const Directions = () => (
  <div>
    <Row label="Sending — 65%">
      <Progress t={{ ...base, dir: "out", status: "active" } as never} />
    </Row>
    <Row label="Receiving — 65%">
      <Progress t={{ ...base, dir: "in", status: "active" } as never} />
    </Row>
  </div>
);

export const States = () => (
  <div>
    <Row label="Completed">
      <Progress t={{ ...base, dir: "out", status: "completed", transferred: base.size } as never} />
    </Row>
    <Row label="Paused">
      <Progress t={{ ...base, dir: "in", status: "paused" } as never} />
    </Row>
    <Row label="Failed">
      <Progress t={{ ...base, dir: "out", status: "failed", transferred: 900_000 } as never} />
    </Row>
    <Row label="Packing — size unknown yet, so motion instead of a stuck bar">
      <Progress t={{ ...base, dir: "out", status: "active", size: 0, transferred: 0 } as never} />
    </Row>
  </div>
);

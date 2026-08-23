// The update cadence, which is the only part of the update flow that can be
// checked without cutting a real release.
//
// Everything here is a decision over a clock and three remembered values, and
// every one of them is a rule about *not* doing something: not asking github.com
// more than once a week, not showing the same banner twice, not interrupting a
// running transfer. A rule that only ever declines is also a rule that fails
// silently when it breaks — the app would simply be chattier, and nobody would
// notice until someone read the access log.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dto, harness, makeIpcMock, resetHarness } from "./mocks";

vi.mock("../ipc", () => makeIpcMock());

import { useStore } from "../store";
import {
  CHECK_INTERVAL_MS,
  dueForCheck,
  isBusy,
  markChecked,
  setAutoCheckEnabled,
  shouldAnnounce,
} from "../update";

const s = () => useStore.getState();

let disposers: (() => void)[] = [];

async function boot() {
  useStore.setState({
    connected: false,
    status: null,
    transfers: {},
    // Reset explicitly: zustand state outlives a test, and a run that loaded
    // the sync summary would otherwise hand it to the next one — which is the
    // difference between "asked for it" and "it was already there".
    sync: null,
    update: {
      phase: "idle",
      info: null,
      progress: null,
      error: null,
      manual: false,
      autoCheck: true,
      dismissed: "",
    },
  });
  disposers.push(await s().init());
}

const RELEASE = { version: "0.13.0", current: "0.12.0", notes: "" };

beforeEach(() => resetHarness());
afterEach(() => {
  disposers.forEach((d) => d());
  disposers = [];
});

describe("when a check is due", () => {
  it("runs the first time, having never checked", () => {
    expect(dueForCheck(Date.now())).toBe(true);
  });

  it("declines for a week after one that landed", () => {
    const t0 = 1_700_000_000_000;
    markChecked(t0);
    expect(dueForCheck(t0)).toBe(false);
    expect(dueForCheck(t0 + CHECK_INTERVAL_MS - 1)).toBe(false);
    expect(dueForCheck(t0 + CHECK_INTERVAL_MS)).toBe(true);
  });

  it("declines outright once the automatic check is turned off", () => {
    setAutoCheckEnabled(false);
    // Not even the first-ever check: the switch is the whole answer.
    expect(dueForCheck(Date.now())).toBe(false);
  });
});

describe("the store honours the cadence", () => {
  it("asks once across both automatic triggers", async () => {
    await boot();
    await s().checkForUpdate(false);
    await s().checkForUpdate(false);
    await s().checkForUpdate(false);
    // Three triggers, one request: the launch check and the weekly timer read
    // the same timestamp, so restarting the app cannot buy another one.
    expect(harness.recorder.checkUpdate).toBe(1);
  });

  it("lets the button through regardless", async () => {
    await boot();
    await s().checkForUpdate(false);
    await s().checkForUpdate(true);
    await s().checkForUpdate(true);
    // An explicit request that quietly declines to do anything is worse than a
    // slow one, so the button never consults the clock.
    expect(harness.recorder.checkUpdate).toBe(3);
  });

  it("does not check automatically when the switch is off", async () => {
    await boot();
    s().setAutoCheck(false);
    await s().checkForUpdate(false);
    expect(harness.recorder.checkUpdate).toBe(0);
    // The button still works — that is the point of leaving it there.
    await s().checkForUpdate(true);
    expect(harness.recorder.checkUpdate).toBe(1);
  });

  it("does not buy itself a week of quiet when the check fails", async () => {
    await boot();
    harness.fail.add("checkUpdate");
    await s().checkForUpdate(false);
    expect(harness.recorder.checkUpdate).toBe(1);
    harness.fail.delete("checkUpdate");
    // The clock is only reset by a check that reached the server, so the next
    // trigger tries again rather than waiting out a week on a failure.
    await s().checkForUpdate(false);
    expect(harness.recorder.checkUpdate).toBe(2);
  });

  it("reports a failure only when the user asked", async () => {
    await boot();
    harness.fail.add("checkUpdate");
    await s().checkForUpdate(false);
    // A laptop closed in a tunnel is not something to interrupt anyone about.
    expect(s().update.phase).toBe("idle");
    expect(s().update.error).toBeNull();

    await s().checkForUpdate(true);
    expect(s().update.phase).toBe("error");
    expect(s().update.error).toContain("checkUpdate");
  });

  it("holds what it found", async () => {
    await boot();
    harness.update = RELEASE;
    await s().checkForUpdate(true);
    expect(s().update.phase).toBe("available");
    expect(s().update.info?.version).toBe("0.13.0");
  });
});

describe("when to actually say something", () => {
  it("says nothing when there is nothing", () => {
    expect(shouldAnnounce(null, "", false)).toBe(false);
  });

  it("announces a release nobody has waved away", () => {
    expect(shouldAnnounce(RELEASE, "", false)).toBe(true);
  });

  it("does not bring back a banner that was closed", () => {
    expect(shouldAnnounce(RELEASE, "0.13.0", false)).toBe(false);
    // …but the next release is a different question.
    expect(shouldAnnounce({ ...RELEASE, version: "0.13.1" }, "0.13.0", false)).toBe(
      true
    );
  });

  it("waits for the board to go quiet", () => {
    expect(shouldAnnounce(RELEASE, "", true)).toBe(false);
  });

  it("remembers the dismissal past this window", async () => {
    await boot();
    harness.update = RELEASE;
    await s().checkForUpdate(true);
    s().dismissUpdate();
    expect(s().update.dismissed).toBe("0.13.0");
    // Written through, not just held: closing the banner has to survive the
    // relaunch, or every start of the app asks again.
    expect(localStorage.getItem("arvolo.updateDismissed")).toBe("0.13.0");
  });
});

describe("a relay that cannot carry the newest release", () => {
  it("keeps what the backend decided, rather than deciding again", async () => {
    await boot();
    // The choice is made in Rust, where the relay probe and the compatibility
    // table are. The store's job is to carry it through to the banner intact —
    // a second opinion here could only ever contradict the first.
    harness.update = {
      version: "0.14.0",
      current: "0.12.0",
      notes: "",
      heldBack: {
        latest: "0.15.0",
        relay: "relay.test",
        relayVersion: "0.14.2",
      },
    };
    await s().checkForUpdate(true);
    expect(s().update.info?.version).toBe("0.14.0");
    expect(s().update.info?.heldBack?.latest).toBe("0.15.0");
    expect(s().update.info?.heldBack?.relay).toBe("relay.test");
  });

  it("installs the version it was offered, not the newest one", async () => {
    await boot();
    harness.update = {
      version: "0.14.0",
      current: "0.12.0",
      notes: "",
      heldBack: { latest: "0.15.0", relay: "relay.test", relayVersion: "0.14.2" },
    };
    await s().checkForUpdate(true);
    s().startUpdate();
    await vi.waitFor(() => expect(harness.recorder.installUpdate).toBe(1));
  });
});

describe("other machines on the same identity", () => {
  it("reads the sync summary when it finds something to announce", async () => {
    await boot();
    // Nothing else in the app loads this: the Devices view is the only other
    // reader, and it may never have been opened. Without it the banner cannot
    // know whether the "update them together" line applies.
    expect(s().sync).toBeNull();
    harness.update = RELEASE;
    await s().checkForUpdate(true);
    expect(s().sync).not.toBeNull();
  });

  it("does not ask when there is nothing to announce", async () => {
    await boot();
    harness.update = null;
    await s().checkForUpdate(true);
    expect(s().sync).toBeNull();
  });
});

describe("what counts as busy", () => {
  it("counts the states where bytes are actually moving", () => {
    expect(isBusy(["active"])).toBe(true);
    expect(isBusy(["preparing"])).toBe(true);
    expect(isBusy(["cancelling"])).toBe(true);
  });

  it("does not count waiting on somebody else", () => {
    // A deposit and a share sit there until another person acts, and both
    // survive a restart untouched. Treating them as busy would mean an app left
    // sharing one file never offers an update again.
    expect(isBusy(["deposited", "sharing", "completed", "stalled"])).toBe(false);
  });

  it("is busy if any single row is", () => {
    expect(isBusy(["completed", "completed", "active"])).toBe(true);
    expect(isBusy([])).toBe(false);
  });
});

describe("installing", () => {
  it("goes through the daemon-stopping command, not the plugin", async () => {
    await boot();
    harness.update = RELEASE;
    await s().checkForUpdate(true);
    s().startUpdate();
    await vi.waitFor(() => expect(harness.recorder.installUpdate).toBe(1));
    // It stays in `downloading`: the real call never resolves, because the
    // process is replaced by the one it just installed.
    expect(s().update.phase).toBe("downloading");
  });

  it("does nothing with nothing to install", async () => {
    await boot();
    s().startUpdate();
    expect(harness.recorder.installUpdate).toBe(0);
  });

  it("shows the progress the backend pushes", async () => {
    await boot();
    harness.updateProgress({ downloaded: 1024, total: 4096 });
    expect(s().update.progress).toEqual({ downloaded: 1024, total: 4096 });
  });

  it("surfaces a refusal rather than sitting on 'downloading' forever", async () => {
    await boot();
    harness.update = RELEASE;
    await s().checkForUpdate(true);
    harness.fail.add("installUpdate");
    s().startUpdate();
    await vi.waitFor(() => expect(s().update.phase).toBe("error"));
    expect(s().update.error).toContain("installUpdate");
  });
});

describe("the board is not disturbed by any of this", () => {
  it("leaves a running transfer alone", async () => {
    harness.snapshot.transfers = [dto.transfer({ id: 1, status: "active" })];
    await boot();
    await vi.waitFor(() => expect(s().transfers.t1).toBeDefined());
    harness.update = RELEASE;
    await s().checkForUpdate(true);
    // The check found something and said so in the store; whether it reaches the
    // screen is `shouldAnnounce`'s call, and with an active row it is a no.
    expect(s().update.info).not.toBeNull();
    expect(
      shouldAnnounce(
        s().update.info,
        s().update.dismissed,
        isBusy(Object.values(s().transfers).map((t) => t.status))
      )
    ).toBe(false);
  });
});

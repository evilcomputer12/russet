// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import { parseLimits, parseUpdates, SetupProtocolError, type ComponentUpdate } from "../../src/setup/contract";
import type { SetupRunOutcome } from "../../src/setup/runner";
import { UpdateWatcher, type UpdatesRunner, type UpdatesUi } from "../../src/setup/updates";

// The live runner's update check on 2026-11-01 (`Russet.Host.exe --setup updates`).
const LIVE =
  '{"version":1,"components":[{"id":"vphone","title":"Virtual iPhone engine","check":' +
  '{"current":"c3b9ecdaf621","upstream":"888d17ca1726","update":true,"staged":"none","reason":"","running":false}}]}';

describe("update check contract", () => {
  it("parses the live runner's line", () => {
    expect(parseUpdates(LIVE)).toEqual([
      {
        id: "vphone", title: "Virtual iPhone engine", current: "c3b9ecdaf621", upstream: "888d17ca1726",
        update: true, staged: "none", running: false,
      },
    ]);
  });

  it("rejects other versions and drops malformed components", () => {
    expect(() => parseUpdates('{"version":2,"components":[]}')).toThrow(SetupProtocolError);
    expect(() => parseUpdates("nope")).toThrow(SetupProtocolError);
    const parsed = parseUpdates(JSON.stringify({
      version: 1,
      components: [
        { id: "vphone", title: "V", check: { current: "not a commit", upstream: "888d17ca1726" } },
        { id: "../x", title: "X", check: { current: "c3b9ecdaf621", upstream: "888d17ca1726" } },
        { id: "vphone", title: "V", check: { current: "c3b9ecdaf621", upstream: "888d17ca1726", staged: "warp", reason: "the\u0000 build" } },
      ],
    }));
    expect(parsed).toHaveLength(1);
    expect(parsed[0]).toMatchObject({ staged: "none", update: false, reason: "the  build" });
  });
});

const BASE: ComponentUpdate = {
  id: "vphone", title: "Virtual iPhone engine", current: "c3b9ecdaf621", upstream: "888d17ca1726",
  update: true, staged: "none", running: false,
};

/** A runner whose checks answer `checks` in turn (the last one repeats) and whose runs answer `outcome`. */
function fakeRunner(checks: ComponentUpdate[][], outcome: SetupRunOutcome = { kind: "done" }) {
  const runs: string[] = [];
  let i = 0;
  const runner: UpdatesRunner = {
    updates: () => Promise.resolve(checks[Math.min(i++, checks.length - 1)]),
    run: (id) => {
      runs.push(id);
      return Promise.resolve(outcome);
    },
  };
  return { runner, runs };
}

function fakeUi(pick?: string) {
  const shown: string[] = [];
  const ui: UpdatesUi = {
    info: (m) => (shown.push(`info: ${m}`), Promise.resolve(pick)),
    warn: (m) => (shown.push(`warn: ${m}`), Promise.resolve(pick)),
    log: () => undefined,
  };
  return { ui, shown };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("UpdateWatcher", () => {
  it("builds an update in the background and reports the switch once", async () => {
    const { runner, runs } = fakeRunner([[BASE], [{ ...BASE, current: BASE.upstream, update: false }]]);
    const { ui, shown } = fakeUi();
    const w = new UpdateWatcher(runner, ui);
    await w.tick();
    await w.tick();
    expect(runs).toEqual(["vphone-update"]);
    expect(shown).toEqual([
      "info: Virtual iPhone engine: updating to 888d17ca1726 in the background; c3b9ecdaf621 stays in use until it is built.",
      "info: Virtual iPhone engine updated to 888d17ca1726 (from c3b9ecdaf621).",
    ]);
  });

  it("rolls back when asked", async () => {
    const { runner, runs } = fakeRunner([[BASE], [{ ...BASE, current: BASE.upstream, update: false }]]);
    const { ui } = fakeUi("Roll Back");
    await new UpdateWatcher(runner, ui).tick();
    await settle();
    expect(runs).toEqual(["vphone-update", "vphone-rollback"]);
  });

  it("reports a failed build once per commit, without rebuilding it", async () => {
    const failed = { ...BASE, staged: "failed" as const, reason: "a Russet patch does not apply" };
    const { runner, runs } = fakeRunner([[failed]]);
    const { ui, shown } = fakeUi();
    const w = new UpdateWatcher(runner, ui);
    await w.tick();
    await w.tick();
    expect(runs).toEqual([]);
    expect(shown).toEqual([
      "warn: Virtual iPhone engine: the update to 888d17ca1726 failed (a Russet patch does not apply). The build in use (c3b9ecdaf621) is unchanged.",
    ]);
  });

  it("retries a failed build when asked", async () => {
    const { runner, runs } = fakeRunner([[{ ...BASE, staged: "failed" }], [BASE]]);
    const { ui } = fakeUi("Retry");
    await new UpdateWatcher(runner, ui).tick();
    await settle();
    expect(runs).toEqual(["vphone-retry-update"]);
  });

  it("waits for a running virtual iPhone, and leaves skipped, building and current commits alone", async () => {
    for (const c of [
      { ...BASE, staged: "built" as const, running: true },
      { ...BASE, staged: "skipped" as const },
      { ...BASE, staged: "building" as const },
      { ...BASE, update: false },
    ]) {
      const { runner, runs } = fakeRunner([[c]]);
      const { ui, shown } = fakeUi();
      const w = new UpdateWatcher(runner, ui);
      await w.tick();
      await w.tick();
      expect(runs).toEqual([]);
      expect(shown.length).toBe(c.running ? 1 : 0);
    }
  });

  it("switches to a built update once nothing runs", async () => {
    const { runner, runs } = fakeRunner([[{ ...BASE, staged: "built" }], [{ ...BASE, current: BASE.upstream, update: false }]]);
    const { ui, shown } = fakeUi();
    await new UpdateWatcher(runner, ui).tick();
    expect(runs).toEqual(["vphone-update"]);
    expect(shown).toEqual(["info: Virtual iPhone engine updated to 888d17ca1726 (from c3b9ecdaf621)."]);
  });
});

describe("WSL limits", () => {
  // The live runner's report on 2026-11-01 (`Russet.Host.exe --setup limits`).
  const LIMITS =
    '{"version":1,"machineMemoryBytes":34265411584,"memoryGB":24,"wslMemoryBytes":25202298880,"restartNeeded":false,' +
    '"diskGB":200,"diskBytes":136004501504,"diskUsedBytes":125489614848,"freeBytes":209018105856}';

  it("parses the live runner's report and drops what it cannot read", () => {
    expect(parseLimits(LIMITS)).toEqual({
      machineMemoryBytes: 34265411584, memoryGB: 24, wslMemoryBytes: 25202298880, restartNeeded: false,
      diskGB: 200, diskBytes: 136004501504, diskUsedBytes: 125489614848, freeBytes: 209018105856,
    });
    expect(parseLimits('{"version":1,"machineMemoryBytes":1,"memoryGB":null,"diskGB":64,"freeBytes":-5}')).toEqual({
      machineMemoryBytes: 1, restartNeeded: false, diskGB: 64,
    });
    expect(() => parseLimits('{"version":2,"machineMemoryBytes":1,"diskGB":64}')).toThrow(SetupProtocolError);
    expect(() => parseLimits('{"version":1,"diskGB":64}')).toThrow(SetupProtocolError);
  });

  it("stops updating at the disk limit, says so once, and tries again after the limit changes", async () => {
    const refused: SetupRunOutcome = {
      kind: "failed", code: "setup.vphone-update.disk-limit", retryable: true,
      message: "An update needs about 25 GB more and would grow Russet's disk to 230 GB, past its 200 GB limit.",
    };
    const { runner, runs } = fakeRunner([[BASE]], refused);
    const shown: string[] = [];
    let opened = 0;
    const answer: { pick?: string } = {};
    const ui: UpdatesUi = {
      info: (m) => (shown.push(`info: ${m}`), Promise.resolve(undefined)),
      warn: (m, ...actions) => (shown.push(`warn: ${m} [${actions.join(", ")}]`), Promise.resolve(answer.pick)),
      log: () => undefined,
      changeLimits: () => (opened++, Promise.resolve()),
    };
    const w = new UpdateWatcher(runner, ui);
    await w.tick();
    await settle();
    await w.tick();
    await w.tick();
    expect(runs).toEqual(["vphone-update"]);
    expect(shown.filter((m) => m.startsWith("warn"))).toEqual([
      "warn: Virtual iPhone engine: An update needs about 25 GB more and would grow Russet's disk to 230 GB, past its 200 GB limit. [Change WSL Limits, Retry]",
    ]);

    // The owner changes the limit from the notice: the next check builds again.
    const again = fakeRunner([[BASE]], refused);
    answer.pick = "Change WSL Limits";
    const w2 = new UpdateWatcher(again.runner, ui);
    await w2.tick();
    await settle();
    expect(opened).toBe(1);
    await w2.tick();
    expect(again.runs).toEqual(["vphone-update", "vphone-update"]);
  });
});

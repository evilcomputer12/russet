// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it, vi } from "vitest";
import type { SetupRunEvent, SetupStatus, SetupStepState } from "../../src/setup/contract";
import { SetupModel, type DaemonProbe, type SetupRunnerLike } from "../../src/setup/model";
import type { SetupRunOutcome } from "../../src/setup/runner";
import type { XipCandidate } from "../../src/setup/xcode";

/** A scripted runner: status from a mutable map; run outcomes per step. */
class ScriptedRunner implements SetupRunnerLike {
  readonly states = new Map<string, SetupStepState>([
    ["wsl", "done"], ["backend", "done"], ["apple-devices", "done"], ["iphone", "unavailable"],
    ["xtool", "done"], ["xcode-sdk", "needed"], ["vphone", "needed"], ["virtual-iphone", "unavailable"],
  ]);
  readonly runs: Array<{ id: string; args: Record<string, string> }> = [];
  outcomes = new Map<string, SetupRunOutcome>();
  hold?: { resolve: () => void };

  async status(): Promise<SetupStatus> {
    return { version: 1, steps: [...this.states].map(([id, state]) => ({ id, state, title: `Title ${id}` })) };
  }

  async run(id: string, args: Readonly<Record<string, string>>, onEvent: (e: SetupRunEvent) => void, signal?: AbortSignal): Promise<SetupRunOutcome> {
    this.runs.push({ id, args: { ...args } });
    onEvent({ step: id, event: "progress", message: "Working", fraction: 0.4 });
    onEvent({ step: id, event: "progress", message: "Working", fraction: 0.5 });
    onEvent({ step: id, event: "log", message: "a log line" });
    if (this.hold) {
      await new Promise<void>((resolve) => {
        this.hold = { resolve };
        signal?.addEventListener("abort", () => resolve());
      });
      if (signal?.aborted) {
        return { kind: "cancelled" };
      }
    }
    const outcome = this.outcomes.get(id) ?? { kind: "done" };
    if (outcome.kind === "done") {
      this.states.set(id, "done");
      if (id === "vphone") {
        this.states.set("virtual-iphone", "needed");
      }
    } else if (outcome.kind === "failed") {
      this.states.set(id, "failed");
    }
    return outcome;
  }
}

function memento(): { get<T>(k: string): T | undefined; update(k: string, v: unknown): Promise<void>; data: Map<string, unknown> } {
  const data = new Map<string, unknown>();
  return { data, get: <T>(k: string) => data.get(k) as T | undefined, update: async (k, v) => void data.set(k, v) };
}

const XIP: XipCandidate = {
  path: "C:\\Users\\someone\\Downloads\\Xcode_27.2.xip", fileName: "Xcode_27.2.xip", version: "27.2", sizeBytes: 3_500_000_000, mtimeMs: 1,
};

function setup(opts: { xips?: XipCandidate[]; accounts?: number; devices?: ReturnType<NonNullable<DaemonProbe["devices"]>>; runner?: ScriptedRunner | null } = {}) {
  const runner = opts.runner === null ? undefined : (opts.runner ?? new ScriptedRunner());
  const daemon: DaemonProbe = {
    connected: () => true,
    accounts: async () => ({ count: opts.accounts ?? 0, ...(opts.accounts ? { label: "Synthetic Developer" } : {}) }),
    toolchain: async () => ({ swiftOk: true, sdkCount: 0 }),
    devices: () => opts.devices ?? {},
  };
  const store = memento();
  const model = new SetupModel({ runner, daemon, findXips: async () => opts.xips ?? [], store, signInEnabled: () => true });
  return { model, runner, store };
}

describe("SetupModel", () => {
  it("shows the runner steps with the account step before the Xcode step", async () => {
    const { model } = setup({ xips: [XIP] });
    await model.refresh();
    const s = model.snapshot();
    expect(s.steps.map((x) => x.id)).toEqual([
      "wsl", "backend", "apple-devices", "iphone", "xtool", "account", "xcode-sdk", "vphone", "virtual-iphone",
    ]);
    expect(s.steps.find((x) => x.id === "account")).toMatchObject({ state: "needed", actions: ["signIn", "addKey"] });
    expect(s.steps.find((x) => x.id === "wsl")?.actions).toEqual(["repair"]);
    expect(s.steps.find((x) => x.id === "xcode-sdk")?.actions).toEqual(["run"]);
    expect(s.steps.find((x) => x.id === "virtual-iphone")?.actions).toEqual(["recheck"]);
    expect(s.xcode.selected).toBe(XIP.path);
    expect(s.doneCount).toBe(4);
  });

  it("hands the iPhone step to the device picker and completes it from a ready device", async () => {
    const waiting = setup({ devices: { waiting: "iPhone 15 is connected (tap Trust on the iPhone)" } });
    await waiting.model.refresh();
    expect(waiting.model.snapshot().steps.find((x) => x.id === "iphone")).toMatchObject({
      state: "needed", actions: ["pickDevice", "recheck"],
    });
    const ready = setup({ devices: { ready: "iPhone 15, iOS 26.1" } });
    await ready.model.refresh();
    expect(ready.model.snapshot().steps.find((x) => x.id === "iphone")).toMatchObject({
      state: "done", detail: "Ready: iPhone 15, iOS 26.1.", actions: ["pickDevice"],
    });
  });

  it("blocks the Xcode step until an archive is chosen, and passes it as --arg xip", async () => {
    const empty = setup();
    await empty.model.refresh();
    expect(empty.model.snapshot().steps.find((x) => x.id === "xcode-sdk")?.blockedReason).toMatch(/Choose the Xcode archive/);
    await expect(empty.model.runStep("xcode-sdk")).resolves.toMatchObject({ kind: "failed", code: "setup.xcode-sdk.input" });
    expect(empty.runner!.runs).toEqual([]);

    const { model, runner } = setup({ xips: [XIP] });
    await model.refresh();
    await expect(model.runStep("xcode-sdk")).resolves.toEqual({ kind: "done" });
    expect(runner!.runs).toEqual([{ id: "xcode-sdk", args: { xip: XIP.path } }]);
    expect(model.snapshot().steps.find((x) => x.id === "xcode-sdk")?.state).toBe("done");
  });

  it("accepts only archives it found or the user browsed to", async () => {
    const { model } = setup({ xips: [XIP] });
    await model.refresh();
    await model.selectXip("C:\\Windows\\System32\\evil.xip");
    expect(model.selectedXip).toBe(XIP.path);
    await expect(model.browseXip("relative.xip")).resolves.toBe(false);
    await expect(model.browseXip("D:\\Installers\\Xcode_27.1.xip")).resolves.toBe(true);
    expect(model.selectedXip).toBe("D:\\Installers\\Xcode_27.1.xip");
  });

  it("creates the virtual iPhone with the default iPhone17,3 / 27.0.1 or the chosen pairing", async () => {
    const { model, runner } = setup();
    runner!.states.set("virtual-iphone", "needed");
    await model.refresh();
    expect(model.snapshot().virtual).toEqual({ device: "iPhone17,3", ios: "27.0.1" });
    await model.runStep("virtual-iphone");
    expect(runner!.runs.at(-1)).toEqual({ id: "virtual-iphone", args: { device: "iPhone17,3", ios: "27.0.1" } });

    await model.setVirtualChoice({ device: "iPhone17,3", ios: "99.0" });
    expect(model.virtualChoice).toEqual({ device: "iPhone17,3", ios: "27.0.1" });
    await model.setVirtualChoice({ device: "iPhone17,3", ios: "26.6.2" });
    runner!.states.set("virtual-iphone", "needed");
    await model.runStep("virtual-iphone", "repair");
    expect(runner!.runs.at(-1)).toEqual({ id: "virtual-iphone", args: { device: "iPhone17,3", ios: "26.6.2", repair: "1" } });
  });

  it("keeps the failure, offers Retry, and logs the run's output", async () => {
    const { model, runner } = setup();
    runner!.outcomes.set("vphone", { kind: "failed", code: "setup.vphone.disk", message: "Not enough space.", retryable: true });
    await model.refresh();
    await model.runStep("vphone");
    const step = model.snapshot().steps.find((x) => x.id === "vphone")!;
    expect(step).toMatchObject({
      state: "failed",
      error: { code: "setup.vphone.disk", message: "Not enough space.", retryable: true },
      actions: ["retry"],
    });
    expect(step.log).toEqual(["Working", "a log line"]);
  });

  it("runs one step at a time and cancels the running one", async () => {
    const { model, runner } = setup();
    runner!.hold = { resolve: () => {} };
    await model.refresh();
    const first = model.runStep("vphone");
    await new Promise((r) => setTimeout(r, 5));
    const snap = model.snapshot();
    expect(snap.runningStep).toBe("vphone");
    expect(snap.steps.find((x) => x.id === "vphone")).toMatchObject({ state: "running", actions: ["cancel"], progress: { fraction: 0.5 } });
    expect(snap.steps.find((x) => x.id === "wsl")?.blockedReason).toBe("Another step is running.");
    await expect(model.runStep("wsl")).resolves.toBeUndefined();
    model.cancel();
    await expect(first).resolves.toEqual({ kind: "cancelled" });
    expect(model.snapshot().runningStep).toBeUndefined();
  });

  it("runs every remaining step in order and stops at the first failure", async () => {
    const { model, runner } = setup({ xips: [XIP] });
    runner!.outcomes.set("vphone", { kind: "failed", code: "setup.vphone.build", message: "x", retryable: true });
    await model.runAll();
    expect(runner!.runs.map((r) => r.id)).toEqual(["xcode-sdk", "vphone"]);
    // The retry picks up the failed step, then the step it unlocked
    // (virtual-iphone turns "needed" once vphone is done).
    runner!.outcomes.delete("vphone");
    await model.runAll();
    expect(runner!.runs.map((r) => r.id)).toEqual(["xcode-sdk", "vphone", "vphone", "virtual-iphone"]);
    expect(model.contextKeys()["russet.virtualIphoneOk"]).toBe(true);
  });

  it("derives the walkthrough completion keys from real state", async () => {
    const { model, runner } = setup({ xips: [XIP], accounts: 1, devices: { ready: "iPhone 15, iOS 26.1" } });
    await model.refresh();
    expect(model.contextKeys()).toEqual({
      "russet.windowsSetup": true,
      "russet.setupOk": false,
      "russet.toolchainOk": true,
      "russet.sdkOk": false,
      "russet.accountOk": true,
      "russet.virtualIphoneOk": false,
    });
    for (const id of ["xcode-sdk", "vphone", "virtual-iphone"]) {
      runner!.states.set(id, "done");
    }
    await model.refresh();
    expect(model.contextKeys()).toMatchObject({ "russet.setupOk": true, "russet.sdkOk": true, "russet.virtualIphoneOk": true });
  });

  it("without a runner, shows the engine-backed steps only and never claims setup", async () => {
    const { model } = setup({ runner: null });
    await model.refresh();
    const s = model.snapshot();
    expect(s.runnerAvailable).toBe(false);
    expect(s.steps.map((x) => x.id)).toEqual(["toolchain", "account"]);
    expect(model.contextKeys()["russet.setupOk"]).toBe(false);
  });

  it("reports a runner that cannot answer", async () => {
    const runner = new ScriptedRunner();
    runner.status = vi.fn().mockRejectedValue(new Error("The Russet setup runner is not installed."));
    const { model } = setup({ runner });
    await model.refresh();
    expect(model.snapshot().runnerError).toMatch(/not installed/);
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { EventEmitter } from "node:events";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { PassThrough } from "node:stream";
import { afterEach, describe, expect, it } from "vitest";
import type { SetupRunEvent } from "../../src/setup/contract";
import { SetupRunner, type RunnerProcess, type SpawnFn } from "../../src/setup/runner";

const FAKE = path.resolve(__dirname, "../fixtures/fake-setup-runner.mjs");

/** An in-process child whose output a test scripts line by line. */
class FakeChild extends EventEmitter {
  readonly stdout = new PassThrough();
  readonly stderr = new PassThrough();
  killed = false;
  kill(): boolean {
    this.killed = true;
    setImmediate(() => this.emit("exit", null));
    return true;
  }
  line(obj: unknown): void {
    this.stdout.write(`${typeof obj === "string" ? obj : JSON.stringify(obj)}\n`);
  }
}

function fakeSpawn(): { spawn: SpawnFn; calls: string[][]; child: () => FakeChild } {
  const calls: string[][] = [];
  let last: FakeChild | undefined;
  const spawn: SpawnFn = (_file, args) => {
    calls.push(args);
    last = new FakeChild();
    return last as unknown as RunnerProcess;
  };
  return { spawn, calls, child: () => last! };
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 5));

describe("SetupRunner with an in-process fake", () => {
  it("reads one status line", async () => {
    const f = fakeSpawn();
    const runner = new SetupRunner({ file: "C:\\Russet\\host\\Russet.Host.exe", prefix: [] }, f.spawn);
    const p = runner.status();
    await tick();
    f.child().line({ version: 1, steps: [{ id: "wsl", state: "done", title: "WSL" }] });
    await expect(p).resolves.toEqual({ version: 1, steps: [{ id: "wsl", state: "done", title: "WSL" }] });
    expect(f.calls[0]).toEqual(["--setup", "status"]);
  });

  it("times out a silent status and kills the runner", async () => {
    const f = fakeSpawn();
    const runner = new SetupRunner({ file: "x", prefix: [] }, f.spawn);
    await expect(runner.status(30)).rejects.toThrow(/in time/);
    expect(f.child().killed).toBe(true);
  });

  it("streams progress and resolves done", async () => {
    const f = fakeSpawn();
    const runner = new SetupRunner({ file: "x", prefix: [] }, f.spawn);
    const events: SetupRunEvent[] = [];
    const p = runner.run("vphone", {}, (e) => events.push(e));
    await tick();
    const c = f.child();
    c.line({ step: "vphone", event: "progress", message: "Fetching", fraction: 0.1 });
    c.stdout.write('{"step":"vphone","event":"log",');
    c.stdout.write('"message":"half a line"}\r\n');
    c.line({ step: "vphone", event: "done" });
    c.emit("exit", 0);
    await expect(p).resolves.toEqual({ kind: "done" });
    expect(events.map((e) => e.event)).toEqual(["progress", "log", "done"]);
    expect(f.calls[0]).toEqual(["--setup", "run", "vphone"]);
  });

  it("reports a failed event, an exit without an outcome, a protocol error and a spawn error", async () => {
    const f = fakeSpawn();
    const runner = new SetupRunner({ file: "x", prefix: [] }, f.spawn);

    let p = runner.run("xtool", {}, () => {});
    await tick();
    f.child().line({ step: "xtool", event: "failed", code: "setup.xtool.network", message: "Offline", retryable: true });
    f.child().emit("exit", 1);
    await expect(p).resolves.toEqual({ kind: "failed", code: "setup.xtool.network", message: "Offline", retryable: true });

    p = runner.run("xtool", {}, () => {});
    await tick();
    f.child().emit("exit", 3);
    await expect(p).resolves.toMatchObject({ kind: "failed", code: "setup.xtool.interrupted" });

    p = runner.run("xtool", {}, () => {});
    await tick();
    f.child().line("garbage");
    await expect(p).resolves.toMatchObject({ kind: "failed", code: "setup.xtool.protocol" });
    expect(f.child().killed).toBe(true);

    p = runner.run("xtool", {}, () => {});
    await tick();
    f.child().emit("error", Object.assign(new Error("spawn"), { code: "ENOENT" }));
    await expect(p).resolves.toMatchObject({ kind: "failed", code: "setup.xtool.runner-unavailable", message: expect.stringMatching(/not installed/) });
  });

  it("cancels by killing the runner", async () => {
    const f = fakeSpawn();
    const runner = new SetupRunner({ file: "x", prefix: [] }, f.spawn);
    const abort = new AbortController();
    const p = runner.run("virtual-iphone", { device: "iPhone17,3", ios: "27.0.1" }, () => {}, abort.signal);
    await tick();
    abort.abort();
    await expect(p).resolves.toEqual({ kind: "cancelled" });
    expect(f.child().killed).toBe(true);
    expect(f.calls[0]).toEqual(["--setup", "run", "virtual-iphone", "--arg", "device=iPhone17,3", "--arg", "ios=27.0.1"]);
  });
});

describe("SetupRunner.fromEnvironment", () => {
  it("uses RUSSET_WINDOWS_HOST on Windows only, and the script override anywhere", () => {
    expect(SetupRunner.fromEnvironment({ RUSSET_WINDOWS_HOST: "C:\\Russet\\host\\Russet.Host.exe" }, "win32")?.command)
      .toEqual({ file: "C:\\Russet\\host\\Russet.Host.exe", prefix: [] });
    expect(SetupRunner.fromEnvironment({ RUSSET_WINDOWS_HOST: "relative\\host.exe" }, "win32")).toBeUndefined();
    expect(SetupRunner.fromEnvironment({ RUSSET_WINDOWS_HOST: "C:\\Russet\\host\\Russet.Host.exe" }, "linux")).toBeUndefined();
    expect(SetupRunner.fromEnvironment({ RUSSET_SETUP_RUNNER: FAKE }, "linux", "/usr/bin/node")?.command).toEqual({
      file: "/usr/bin/node", prefix: [FAKE], env: { ELECTRON_RUN_AS_NODE: "1" },
    });
  });
});

describe("SetupRunner against the fake runner script", () => {
  let dir: string | undefined;
  afterEach(() => {
    if (dir) {
      fs.rmSync(dir, { recursive: true, force: true });
      dir = undefined;
    }
    delete process.env.FAKE_SETUP_STATE;
    delete process.env.FAKE_SETUP_FAIL;
  });

  const runner = (): SetupRunner => SetupRunner.fromEnvironment({ RUSSET_SETUP_RUNNER: FAKE }, process.platform, process.execPath)!;

  it("reports status, runs a step to done and persists it", async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "russet-w19-"));
    process.env.FAKE_SETUP_STATE = path.join(dir, "state.json");
    process.env.FAKE_SETUP_DELAY = "1";
    const r = runner();
    const before = await r.status();
    expect(before.steps.find((s) => s.id === "vphone")?.state).toBe("needed");
    const events: SetupRunEvent[] = [];
    await expect(r.run("vphone", {}, (e) => events.push(e))).resolves.toEqual({ kind: "done" });
    expect(events.filter((e) => e.event === "progress").length).toBeGreaterThan(3);
    const after = await r.status();
    expect(after.steps.find((s) => s.id === "vphone")?.state).toBe("done");
    expect(after.steps.find((s) => s.id === "virtual-iphone")?.state).toBe("needed");
    // A second run of a done step is a no-op done.
    const again: SetupRunEvent[] = [];
    await expect(r.run("vphone", {}, (e) => again.push(e))).resolves.toEqual({ kind: "done" });
    expect(again).toEqual([{ step: "vphone", event: "done" }]);
  });

  it("fails bad arguments without retry, and a backend error with retry", async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "russet-w19-"));
    process.env.FAKE_SETUP_STATE = path.join(dir, "state.json");
    process.env.FAKE_SETUP_FAIL = "vphone";
    const r = runner();
    await expect(r.run("xcode-sdk", {}, () => {})).resolves.toMatchObject({
      kind: "failed", code: "setup.xcode-sdk.bad-args", retryable: false,
    });
    await expect(r.run("vphone", {}, () => {})).resolves.toMatchObject({
      kind: "failed", code: "setup.vphone.backend-error", retryable: true,
    });
  });
});

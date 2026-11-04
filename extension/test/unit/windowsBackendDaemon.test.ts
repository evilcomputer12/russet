// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { EventEmitter } from "node:events";
import { describe, it, expect, vi, beforeEach } from "vitest";

const spawned: { args: string[]; child: EventEmitter & { kill: () => void } }[] = [];
vi.mock("node:child_process", () => ({
  spawn: (_file: string, args: string[]) => {
    const child = Object.assign(new EventEmitter(), { kill: vi.fn() });
    spawned.push({ args, child });
    return child;
  },
}));

const { WindowsBackend } = await import("../../src/daemon/windowsBackend");

describe("managed Windows backend: starting a stopped daemon", () => {
  beforeEach(() => { spawned.length = 0; });

  it("asks the host to start the daemon, and shares a start in progress", async () => {
    const backend = new WindowsBackend("C:\\Russet\\host\\Russet.Host.exe", "/home/russet/.russet/ide");
    const first = backend.startDaemon();
    const second = backend.startDaemon();
    expect(spawned).toHaveLength(1);
    expect(spawned[0].args).toEqual(["--setup", "daemon-start"]);
    spawned[0].child.emit("exit", 0);
    await expect(first).resolves.toBe(true);
    await expect(second).resolves.toBe(true);

    // Once it has ended, the next call starts again; a host that fails reports false.
    const third = backend.startDaemon();
    expect(spawned).toHaveLength(2);
    spawned[1].child.emit("exit", 1);
    await expect(third).resolves.toBe(false);
  });

  it("reports false when the host cannot run, and starts nothing once disposed", async () => {
    const backend = new WindowsBackend("C:\\Russet\\host\\Russet.Host.exe", "/home/russet/.russet/ide");
    const started = backend.startDaemon();
    spawned[0].child.emit("error", new Error("ENOENT"));
    await expect(started).resolves.toBe(false);
    backend.dispose();
    await expect(backend.startDaemon()).resolves.toBe(false);
    expect(spawned).toHaveLength(1);
  });
});

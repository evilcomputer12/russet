// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, it, expect, vi } from "vitest";
import { WindowsBackend } from "../../src/daemon/windowsBackend";
import { RussetDebugAdapterFactory, stashPrepared, dropPrepared } from "../../src/debug/adapterFactory";
import type { DebugPrepareResult } from "../../src/generated/protocol";
import type { DebugSession } from "vscode";

vi.mock("vscode", () => ({
  DebugAdapterNamedPipeServer: class {
    constructor(readonly path: string) {}
  },
}));

describe("managed Windows backend", () => {
  it("rejects a workspace-relative supervisor and a runtime escape", async () => {
    expect(() => new WindowsBackend(".\\host.exe", "/home/russet/app")).toThrow(/absolute Windows/);
    const backend = new WindowsBackend("C:\\Russet\\Russet.Host.exe", "/home/russet/app");
    await expect(backend.open("/home/russet/other/dap.sock")).rejects.toThrow(/outside/);
    await expect(backend.open("/home/russet/app/runtime/../other/dap.sock")).rejects.toThrow(/outside/);
  });

  it("returns the supervisor's Windows endpoint for a Linux debug session and releases it", async () => {
    const dispose = vi.fn();
    const open = vi.fn().mockResolvedValue({ endpoint: "\\\\.\\pipe\\russet-dev-test", dispose });
    const backend = { open } as unknown as WindowsBackend;
    const factory = new RussetDebugAdapterFactory(backend);
    const token = stashPrepared({ dap: { transport: "unix", path: "/home/russet/app/runtime/sessions/test/dap.sock" } } as DebugPrepareResult);
    const session = { id: "test-session", configuration: { __russetToken: token } } as unknown as DebugSession;
    try {
      const adapter = await factory.createDebugAdapterDescriptor(session);
      expect(open).toHaveBeenCalledWith("/home/russet/app/runtime/sessions/test/dap.sock", "dap");
      expect((adapter as unknown as { path: string }).path).toBe("\\\\.\\pipe\\russet-dev-test");
      factory.release(session.id);
      expect(dispose).toHaveBeenCalledOnce();
    } finally { factory.dispose(); dropPrepared(token); }
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
import { expect, it, vi } from "vitest";
import type { RussetClient } from "../../src/daemon/client";
import type { Store } from "../../src/state/store";
import { RussetDebugConfigProvider } from "../../src/debug/configProvider";

vi.mock("vscode", () => ({
  ProgressLocation: { Notification: 15 },
  window: { withProgress: async (_options: unknown, task: () => Promise<unknown>) => task() },
}));

it("keeps the Russet descriptor factory when a real-daemon-shaped DAP recipe names lldb-dap", async () => {
  const call = vi.fn().mockResolvedValue({
    sessionId: "synthetic-session", bundleId: "dev.russet.synthetic",
    initCommands: [], attachCommands: [],
    dapConfig: { type: "lldb-dap", request: "attach", attachCommands: ["!prepared attach"] },
  });
  const client = { state: "connected", call } as unknown as RussetClient;
  const store = { state: { project: { root: "C:\\Synthetic" } }, setRun: vi.fn() } as unknown as Store;
  const provider = new RussetDebugConfigProvider(client, store);
  const result = await provider.resolveDebugConfigurationWithSubstitutedVariables(undefined, {
    type: "russet", request: "launch", name: "Debug on iPhone", symbols: "none",
  });
  expect(result?.type).toBe("russet");
  expect(result?.request).toBe("attach");
  expect(result?.attachCommands).toEqual(["!prepared attach"]);
  expect(call).toHaveBeenCalledWith("debug.prepare", expect.objectContaining({ frontend: "dap", symbols: "none" }), undefined);
});

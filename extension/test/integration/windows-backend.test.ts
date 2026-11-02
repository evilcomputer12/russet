// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
// Actual Windows extension host and real Rust daemon; device backend is none.
import * as vscode from "vscode";
import * as assert from "node:assert/strict";

interface Api {
  client: {
    state: string;
    call(method: string, params: unknown): Promise<Record<string, unknown>>;
  };
}

export async function run(): Promise<void> {
  console.log("Managed supervisor environment present:", Boolean(process.env.RUSSET_WINDOWS_HOST), Boolean(process.env.RUSSET_BACKEND_HOME));
  const extension = vscode.extensions.getExtension("russet.russet");
  assert.ok(extension);
  const api = await extension.activate() as Api;
  const until = Date.now() + 40000;
  while (api.client.state !== "connected" && Date.now() < until) {
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.equal(api.client.state, "connected", "actual extension connects through the Windows supervisor");
  assert.deepEqual(await api.client.call("daemon.ping", {}), {});
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  assert.ok(root);
  const detected = await api.client.call("project.detect", { dir: root });
  assert.equal(detected.stack, "swiftpm");
  assert.equal(String(detected.root).toLowerCase(), root.toLowerCase());
  console.log("PASS Windows extension -> host pipe -> WSL -> Rust daemon, project.detect and ping");
  if (process.env.RUSSET_TEST_HOLD_UI === "1") {
    await vscode.commands.executeCommand("russet.devices.focus");
    await new Promise((resolve) => setTimeout(resolve, 120000));
  }
}

// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// EXT-02/EXT-03/EXT-06 integration: connect to the (fake) daemon, list the device,
// and the F5 debug flow — debug.prepare → DebugAdapterNamedPipeServer → a `stopped`
// event at the breakpoint. DC-14 covers the same path on a real iPhone.

import * as vscode from "vscode";
import * as assert from "node:assert";
import * as path from "node:path";
import { FakeDaemon } from "./fake-daemon";

interface TestApi {
  client: { state: string };
  getActiveDevice(): { osVersion?: string; udid?: string } | undefined;
}

async function waitFor(cond: () => boolean, ms: number, label: string): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (cond()) {
      return;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`timed out waiting for ${label}`);
}

function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error(`timed out: ${label}`)), ms)),
  ]);
}

describe("Russet extension", () => {
  // The Mocha timeout is set in .vscode-test.mjs (60s).
  let daemon: FakeDaemon;
  let api: TestApi;

  before(async () => {
    const home = process.env.RUSSET_HOME;
    assert.ok(home, "RUSSET_HOME must be set by the launcher");
    const sock = path.join(home, "runtime", "russetd.sock");
    const root = vscode.workspace.workspaceFolders?.[0].uri.fsPath;
    assert.ok(root, "a workspace folder must be open");
    daemon = new FakeDaemon({ socketPath: sock, projectRoot: root });
    await daemon.listen();

    const ext = vscode.extensions.getExtension("russet.russet");
    assert.ok(ext, "the Russet extension must be present");
    api = (await ext.activate()) as TestApi;
    await waitFor(() => api.client.state === "connected", 25000, "daemon connection");
  });

  after(() => {
    daemon?.dispose();
  });

  it("connects and reports the device from device.list", async () => {
    await waitFor(() => !!api.getActiveDevice(), 10000, "active device");
    const device = api.getActiveDevice();
    assert.ok(device, "an active device is selected");
    assert.strictEqual(device?.osVersion, "27.0");
  });

  it("resolves the russet debug type and stops at the breakpoint (F5)", async () => {
    const folder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(folder);

    // Set a breakpoint so setBreakpoints carries a line the mock verifies.
    const source = vscode.Uri.file(path.join(folder.uri.fsPath, "Sources/Hello/ContentView.swift"));
    vscode.debug.addBreakpoints([
      new vscode.SourceBreakpoint(new vscode.Location(source, new vscode.Position(5, 0))),
    ]);

    const stopped = new Promise<void>((resolve) => {
      const tracker = vscode.debug.registerDebugAdapterTrackerFactory("russet", {
        createDebugAdapterTracker() {
          return {
            onDidSendMessage(message: unknown) {
              const m = message as { type?: string; event?: string };
              if (m.type === "event" && m.event === "stopped") {
                tracker.dispose();
                resolve();
              }
            },
          };
        },
      });
    });

    const ok = await vscode.debug.startDebugging(folder, {
      type: "russet",
      request: "launch",
      name: "Run on iPhone",
      target: "Hello",
      device: "auto",
      configuration: "Debug",
      stopAtEntry: false,
      build: true,
    });
    assert.ok(ok, "startDebugging(russet) returned true");

    await withTimeout(stopped, 25000, "stopped event at the breakpoint");
    await vscode.debug.stopDebugging();
  });
});

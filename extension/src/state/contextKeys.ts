// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Mirrors the store into VS Code `setContext` keys (ide.md §5.3), which drive
// when-clauses in the manifest. Later-feature keys are set to false so their
// surfaces stay hidden until their card ships.

import * as vscode from "vscode";
import type { Store } from "./store";
import type { RussetClient, ConnectionState } from "../daemon/client";

async function setKey(key: string, value: unknown): Promise<void> {
  await vscode.commands.executeCommand("setContext", key, value);
}

export function installContextKeys(
  store: Store,
  client: RussetClient,
  disposables: vscode.Disposable[],
): void {
  const isRussetIde = vscode.env.appName === "Russet";
  void setKey("russet.isRussetIde", isRussetIde);

  // Feature flags for surfaces owned by later cards (hidden by when-clauses).
  void setKey("russet.featureAgents", false);
  void setKey("russet.featureMirror", true);

  const applyConnection = (state: ConnectionState): void => {
    void setKey("russet.daemonConnected", state === "connected");
  };
  applyConnection(client.state);
  disposables.push(client.onDidChangeState(applyConnection));

  const apply = (): void => {
    const s = store.state;
    void setKey("russet.projectKind", s.project?.stack);
    void setKey("russet.hasDevice", !!store.activeDevice);
    void setKey("russet.deviceReady", store.activeDevice?.state === "ready");
    const running = s.run.state !== "idle" && s.run.state !== "exited" &&
      s.run.state !== "crashed" && s.run.state !== "failed" && s.run.state !== "lost";
    void setKey("russet.isRunning", running);
    void setKey("russet.isBuilding", s.build.phase === "building");
    void setKey(
      "russet.isDebugging",
      s.run.isDebug && (s.run.state === "running" || s.run.state === "ready" || s.run.state === "stopped"),
    );
    void setKey("russet.canHotReload", store.canHotReload);
    void setKey("russet.pendingApprovals", 0);
  };

  apply();
  disposables.push(store.onDidChange(apply));
}

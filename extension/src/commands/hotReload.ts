// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Hot Reload / Hot Restart (EXT-07). Delegates to the daemon's run.hotReload for the
// active session; the button is gated by russet.canHotReload (ide.md §5.7).

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import { showDaemonError } from "../daemon/errors";

export async function hotReload(
  client: RussetClient,
  store: Store,
  kind: "reload" | "restart",
): Promise<void> {
  const runId = store.state.run.sessionId;
  if (!runId) {
    void vscode.window.showInformationMessage("Start the app before using Hot Reload.");
    return;
  }
  if (store.state.project?.stack === "flutter") {
    // Flutter reloads go through Dart-Code's own commands (ide.md §5.7).
    await vscode.commands.executeCommand(
      kind === "reload" ? "flutter.hotReload" : "flutter.hotRestart",
    );
    return;
  }
  try {
    const result = (await client.call(
      "run.hotReload" as never,
      { runId, kind } as never,
    )) as { supported?: boolean; durationMs?: number; message?: string };
    if (result?.supported === false) {
      void vscode.window.showWarningMessage(
        result.message ?? "Hot reload is not supported for this project.",
      );
    } else if (result?.durationMs !== undefined) {
      vscode.window.setStatusBarMessage(
        `$(zap) ${kind === "reload" ? "Reloaded" : "Restarted"} in ${(result.durationMs / 1000).toFixed(1)} s`,
        3000,
      );
    }
  } catch (err) {
    await showDaemonError(err, "Hot Reload failed");
  }
}

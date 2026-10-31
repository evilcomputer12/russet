// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A DAP tracker for `russet` sessions (EXT-06): it reflects stop/continue into the
// store (so the status bar and context keys follow), cleans up the daemon session
// on terminate (run.stop detach), and drops the stashed adapter descriptor.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import { dropPrepared } from "./adapterFactory";

interface DapMessage {
  type?: string;
  event?: string;
  command?: string;
  body?: unknown;
}

export class RussetDebugTrackerFactory implements vscode.DebugAdapterTrackerFactory {
  constructor(
    private readonly client: RussetClient,
    private readonly store: Store,
  ) {}

  createDebugAdapterTracker(session: vscode.DebugSession): vscode.DebugAdapterTracker {
    const cfg = session.configuration as { __russetToken?: string; __russetSessionId?: string };
    const sessionId = cfg.__russetSessionId;
    const client = this.client;
    const store = this.store;

    return {
      onDidSendMessage(message: DapMessage): void {
        if (message.type !== "event") {
          return;
        }
        switch (message.event) {
          case "stopped":
            store.setRun({ ...store.state.run, state: "stopped", isDebug: true, sessionId });
            break;
          case "continued":
            store.setRun({ ...store.state.run, state: "running", isDebug: true, sessionId });
            break;
          case "terminated":
          case "exited":
            store.setRun({ state: "exited", isDebug: true, sessionId });
            break;
          default:
            break;
        }
      },
      onExit(): void {
        dropPrepared(cfg.__russetToken);
        store.setRun({ state: "idle", isDebug: false });
        if (sessionId && client.state === "connected") {
          void client.call("run.stop", { sessionId, mode: "detach" }).catch(() => {
            /* best effort cleanup */
          });
        }
      },
    };
  }
}

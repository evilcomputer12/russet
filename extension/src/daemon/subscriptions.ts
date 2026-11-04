// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Wires the daemon notification stream into the UI (EXT-02, EXT-03, EXT-05, EXT-08).
// Handlers are registered once and survive reconnects (the client replays them);
// the subscriptions themselves are re-issued whenever the daemon connects.

import * as vscode from "vscode";
import type { RussetClient } from "./client";
import type { Store } from "../state/store";
import type { ConsoleViewProvider } from "../panels/consoleView";
import type { Diagnostics } from "../run/diagnostics";
import type { RunManager } from "../run/session";
import type { BuildLog } from "../run/buildLog";
import { log } from "../util/log";

export interface SubscriptionDeps {
  client: RussetClient;
  store: Store;
  console: ConsoleViewProvider;
  diagnostics: Diagnostics;
  run: RunManager;
  /** The Russet Build output and the build/run progress. */
  buildLog?: BuildLog;
}

export function installSubscriptions(deps: SubscriptionDeps, disposables: vscode.Disposable[]): void {
  const { client, store, console: consoleView, diagnostics, run, buildLog } = deps;

  disposables.push(
    client.onEvent("device.changed", (p) => {
      store.upsertDevice(p.device, p.change === "removed");
    }),
  );

  disposables.push(
    client.onEvent("run.state", (p) => {
      const current = store.state.run;
      if (!current.sessionId || current.sessionId === p.sessionId) {
        store.setRun({ ...current, state: p.state, sessionId: p.sessionId });
      }
    }),
  );

  disposables.push(
    client.onEvent("run.output", (p) => {
      consoleView.append(p.entries);
    }),
  );

  disposables.push(
    client.onEvent("run.exited", (p) => {
      run.onSessionEnded(p.sessionId);
      if (store.state.run.sessionId === p.sessionId) {
        store.setRun({ state: p.state ?? "exited", isDebug: store.state.run.isDebug, sessionId: p.sessionId });
      }
    }),
  );

  disposables.push(
    client.onEvent("app.output", (p) => {
      consoleView.appendRaw([p.data], "stdout");
    }),
  );

  disposables.push(
    client.onEvent("logs.entries", (p) => {
      consoleView.append(p.entries);
    }),
  );

  // Build, run and debug jobs: their output goes to the Russet Build output (the console is the app's).
  disposables.push(
    client.onEvent("job.updated", (p) => {
      buildLog?.update(p.job);
    }),
  );
  disposables.push(
    client.onEvent("job.events", (p) => {
      for (const ev of p.events) {
        if (ev.kind === "diagnostic") {
          diagnostics.add(ev);
        }
      }
      buildLog?.events(p.jobId, p.events);
    }),
  );

  // Re-issue subscriptions on every (re)connect.
  disposables.push(
    client.onDidChangeState((state) => {
      if (state === "connected") {
        void subscribeAll(deps);
      }
    }),
  );
  if (client.state === "connected") {
    void subscribeAll(deps);
  }
}

async function subscribeAll(deps: SubscriptionDeps): Promise<void> {
  const { client, store } = deps;
  try {
    await client.call("device.watch", {} as never);
    await client.call("job.subscribe", { includeEvents: true });
    await client.call("run.subscribe", {});
  } catch (err) {
    log().warn(`[daemon] subscribe failed: ${String(err)}`);
  }
  await refreshDevices(deps);
  await refreshProject(store, client);
}

export async function refreshDevices(deps: SubscriptionDeps): Promise<void> {
  try {
    const result = await deps.client.call("device.list", {} as never);
    deps.store.setDevices(result.devices);
  } catch (err) {
    log().warn(`[daemon] device.list failed: ${String(err)}`);
  }
}

export async function refreshProject(store: Store, client: RussetClient): Promise<void> {
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!root) {
    store.setProject(undefined);
    return;
  }
  try {
    const info = await client.call("project.info", { project: root });
    store.setProject(info);
  } catch {
    // Not a Russet-recognised project; leave it unset.
    store.setProject(undefined);
  }
}

// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// RunManager: the Run and Stop flow (EXT-05, ide.md §5.7). Build → install →
// launch through `run.start`; Stop through `run.stop`. Debug launches go through
// the `russet` debug type (see src/debug/), not here.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import type { ConsoleViewProvider } from "../panels/consoleView";
import type { Diagnostics } from "./diagnostics";
import { showDaemonError } from "../daemon/errors";

export function configToWire(label: string | undefined): string {
  return (label ?? "Debug").toLowerCase();
}

export interface RunContext {
  projectRoot: string;
  target?: string;
  configuration: string;
  udid?: string;
}

export class RunManager implements vscode.Disposable {
  private currentSessionId?: string;

  constructor(
    private readonly client: RussetClient,
    private readonly store: Store,
    private readonly console: ConsoleViewProvider,
    private readonly diagnostics: Diagnostics,
  ) {}

  get activeSessionId(): string | undefined {
    return this.currentSessionId;
  }

  resolveContext(): RunContext | undefined {
    const project = this.store.state.project;
    const root = project?.root ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root) {
      void vscode.window.showErrorMessage("Open a project folder before running.");
      return undefined;
    }
    return {
      projectRoot: root,
      target: this.store.state.selectedTarget,
      configuration: configToWire(this.store.state.selectedConfiguration),
      udid: this.store.activeDevice?.udid,
    };
  }

  async run(): Promise<void> {
    if (this.client.state !== "connected") {
      void vscode.window.showErrorMessage("The Russet daemon is not connected.");
      return;
    }
    const ctx = this.resolveContext();
    if (!ctx) {
      return;
    }
    this.diagnostics.reset();
    this.store.setBuild({ phase: "building" });
    const started = Date.now();

    try {
      const result = await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: "Running on iPhone", cancellable: false },
        () =>
          this.client.call("run.start", {
            project: ctx.projectRoot,
            target: ctx.target,
            configuration: ctx.configuration,
            udid: ctx.udid,
            build: true,
          }),
      );
      const errs = this.diagnostics.errorCount;
      this.store.setBuild(
        errs > 0
          ? { phase: "error", errorCount: errs }
          : { phase: "ok", durationMs: Date.now() - started },
      );
      this.currentSessionId = result.sessionId;
      this.store.setRun({
        state: result.sessionId ? "running" : "idle",
        sessionId: result.sessionId,
        isDebug: false,
        bundleId: result.bundleId,
      });
      if (vscode.workspace.getConfiguration("russet").get<boolean>("run.openConsole", true)) {
        await this.console.reveal(result.pid);
      }
    } catch (err) {
      this.store.setBuild({ phase: "error", errorCount: this.diagnostics.errorCount });
      this.store.setRun({ state: "failed", isDebug: false });
      await showDaemonError(err, "Run failed");
    }
  }

  /** Stop the current run session (Run-launched or debug-launched). */
  async stop(): Promise<void> {
    const sessionId = this.currentSessionId ?? this.store.state.run.sessionId;
    if (!sessionId) {
      // A debug session is handled by VS Code's stop; nothing to do here.
      return;
    }
    try {
      await this.client.call("run.stop", { sessionId, mode: "kill" });
    } catch (err) {
      await showDaemonError(err, "Stop failed");
    } finally {
      this.currentSessionId = undefined;
      this.store.setRun({ state: "exited", isDebug: false });
    }
  }

  /** Called from the event router when a session ends. */
  onSessionEnded(sessionId: string): void {
    if (sessionId === this.currentSessionId) {
      this.currentSessionId = undefined;
    }
  }

  dispose(): void {
    /* no owned disposables */
  }
}

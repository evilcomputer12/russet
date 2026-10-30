// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The "russet" task provider (EXT-05): build/run/test/clean/archive tasks run
// through a CustomExecution + Pseudoterminal that drives the daemon. Tier 0 wires
// `build` and `clean`; the pseudoterminal streams job output to the task terminal.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import type { Diagnostics } from "./diagnostics";
import { configToWire } from "./session";
import { showDaemonError, messageOf } from "../daemon/errors";

interface RussetTaskDefinition extends vscode.TaskDefinition {
  type: "russet";
  command: "build" | "run" | "test" | "clean" | "archive";
  target?: string;
  configuration?: "Debug" | "Release";
  device?: string;
}

export class RussetTaskProvider implements vscode.TaskProvider {
  public static readonly taskType = "russet";

  constructor(
    private readonly client: RussetClient,
    private readonly store: Store,
    private readonly diagnostics: Diagnostics,
  ) {}

  provideTasks(): vscode.Task[] {
    const project = this.store.state.project;
    if (!project) {
      return [];
    }
    const configuration = this.store.state.selectedConfiguration as "Debug" | "Release";
    return [
      this.makeTask({ type: "russet", command: "build", configuration }),
      this.makeTask({ type: "russet", command: "clean" }),
    ];
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as RussetTaskDefinition;
    if (def.type !== "russet" || !def.command) {
      return undefined;
    }
    return this.makeTask(def);
  }

  private makeTask(def: RussetTaskDefinition): vscode.Task {
    const task = new vscode.Task(
      def,
      vscode.TaskScope.Workspace,
      `${def.command}${def.target ? ` ${def.target}` : ""}`,
      "russet",
      new vscode.CustomExecution(async () => new RussetPseudoterminal(def, this.client, this.store, this.diagnostics)),
      ["$russet-swiftc"],
    );
    task.group =
      def.command === "build"
        ? vscode.TaskGroup.Build
        : def.command === "clean"
          ? vscode.TaskGroup.Clean
          : undefined;
    return task;
  }
}

class RussetPseudoterminal implements vscode.Pseudoterminal {
  private readonly writeEmitter = new vscode.EventEmitter<string>();
  readonly onDidWrite = this.writeEmitter.event;
  private readonly closeEmitter = new vscode.EventEmitter<number>();
  readonly onDidClose = this.closeEmitter.event;

  constructor(
    private readonly def: RussetTaskDefinition,
    private readonly client: RussetClient,
    private readonly store: Store,
    private readonly diagnostics: Diagnostics,
  ) {}

  open(): void {
    void this.execute();
  }

  close(): void {
    /* nothing to cancel in Tier 0 */
  }

  private line(text: string): void {
    this.writeEmitter.fire(text.replace(/\n/g, "\r\n") + "\r\n");
  }

  private async execute(): Promise<void> {
    const project = this.store.state.project;
    const root = project?.root ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root) {
      this.line("No project folder open.");
      this.closeEmitter.fire(1);
      return;
    }
    try {
      if (this.def.command === "clean") {
        this.line("Cleaning build folder…");
        const result = await this.client.call("build.clean", { project: root, scope: "build" });
        this.line(`Freed ${(result.freedBytes / 1_000_000).toFixed(1)} MB.`);
        this.closeEmitter.fire(0);
        return;
      }
      // build (default)
      this.diagnostics.reset();
      this.store.setBuild({ phase: "building" });
      this.line(`Building ${this.def.target ?? project?.name ?? "project"}…`);
      const started = Date.now();
      const output = await this.client.call("build.start", {
        project: root,
        target: this.def.target,
        configuration: configToWire(this.def.configuration),
      });
      const errs = this.diagnostics.errorCount;
      for (const lint of output.lints) {
        this.line(`lint ${lint.id}: ${lint.message}`);
      }
      this.line(`Built ${output.app.bundleId} in ${(output.durationMs / 1000).toFixed(1)} s.`);
      this.store.setBuild(errs > 0 ? { phase: "error", errorCount: errs } : { phase: "ok", durationMs: Date.now() - started });
      this.closeEmitter.fire(errs > 0 ? 1 : 0);
    } catch (err) {
      this.line(`Build failed: ${messageOf(err)}`);
      this.store.setBuild({ phase: "error", errorCount: this.diagnostics.errorCount });
      void showDaemonError(err, "Build failed");
      this.closeEmitter.fire(1);
    }
  }
}

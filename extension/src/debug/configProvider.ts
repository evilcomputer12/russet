// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Resolves `russet` debug configurations (EXT-06, ide.md §5.7). It calls
// debug.prepare on the daemon — which builds, installs, launches suspended and
// holds the attach — then rewrites the configuration in place into the lldb-dap
// attach recipe the daemon returned, keeping type: "russet".

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import { configToWire } from "../run/session";
import { stashPrepared } from "./adapterFactory";
import { showDaemonError } from "../daemon/errors";
import type { DebugPrepareParams } from "../generated/protocol";

export class RussetDebugConfigProvider implements vscode.DebugConfigurationProvider {
  constructor(
    private readonly client: RussetClient,
    private readonly store: Store,
  ) {}

  /** Offer a default configuration when the user has no launch.json (F5 in an empty folder). */
  provideDebugConfigurations(
    folder: vscode.WorkspaceFolder | undefined,
  ): vscode.DebugConfiguration[] {
    const target = this.store.state.selectedTarget ?? folder?.name ?? "App";
    return [
      {
        type: "russet",
        request: "launch",
        name: "Run on iPhone",
        target,
        device: "auto",
        configuration: "Debug",
        stopAtEntry: false,
        build: true,
      },
    ];
  }

  async resolveDebugConfigurationWithSubstitutedVariables(
    folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration,
    token?: vscode.CancellationToken,
  ): Promise<vscode.DebugConfiguration | undefined | null> {
    // A bare F5 with no configuration: synthesize the default launch.
    if (!config.type && !config.request && !config.name) {
      config = this.provideDebugConfigurations(folder)[0];
    }
    if (config.type !== "russet") {
      return config;
    }
    if (this.client.state !== "connected") {
      void vscode.window.showErrorMessage("The Russet daemon is not connected.");
      return undefined;
    }

    const projectRoot =
      this.store.state.project?.root ??
      folder?.uri.fsPath ??
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!projectRoot) {
      void vscode.window.showErrorMessage("Open a project folder before debugging.");
      return undefined;
    }

    const udid = this.resolveUdid(config.device as string | undefined);

    try {
      const result = await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: "Preparing debug session", cancellable: false },
        () =>
          this.client.call(
            "debug.prepare",
            {
              project: projectRoot,
              target: (config.target as string | undefined) ?? this.store.state.selectedTarget,
              configuration: configToWire((config.configuration as string | undefined) ?? this.store.state.selectedConfiguration),
              udid,
              frontend: "dap",
              stopOnEntry: Boolean(config.stopAtEntry),
              build: config.build !== false,
              args: (config.args as string[] | undefined) ?? [],
              env: (config.env as Record<string, string> | undefined) ?? {},
              symbols: config.symbols as DebugPrepareParams["symbols"],
            },
            token,
          ),
      );

      const dapConfig = (result.dapConfig as Record<string, unknown>) ?? {};
      const token2 = stashPrepared(result);
      this.store.setRun({ state: "attaching", sessionId: result.sessionId, isDebug: true, bundleId: result.bundleId });

      // Rewrite in place to the lldb-dap attach recipe, keeping type "russet".
      return {
        ...config,
        initCommands: result.initCommands,
        attachCommands: result.attachCommands,
        ...dapConfig,
        type: "russet",
        request: "attach",
        name: config.name ?? "Debug on iPhone",
        __russetToken: token2,
        __russetSessionId: result.sessionId,
      };
    } catch (err) {
      await showDaemonError(err, "Could not prepare the debug session");
      return undefined;
    }
  }

  private resolveUdid(device: string | undefined): string | undefined {
    if (device && device !== "auto") {
      return device;
    }
    return this.store.activeDevice?.udid;
  }
}

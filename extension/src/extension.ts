// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Russet extension entry point (ide.md §5.2). activate() wires the daemon
// client, state, views, status bar, run/debug flow, console, the embedded
// iPhone panel and the guided setup, sign-in and run-destination surfaces
// (W19), and returns the RussetApi other extensions consume (§5.4).

import * as vscode from "vscode";
import { RussetClient } from "./daemon/client";
import { WindowsBackend } from "./daemon/windowsBackend";
import { Store } from "./state/store";
import { installContextKeys } from "./state/contextKeys";
import {
  installSubscriptions,
  refreshDevices,
  refreshProject,
} from "./daemon/subscriptions";
import { Diagnostics } from "./run/diagnostics";
import { RunManager } from "./run/session";
import { RussetTaskProvider } from "./run/taskProvider";
import { ConsoleViewProvider } from "./panels/consoleView";
import { MirrorViewProvider } from "./panels/mirrorView";
import { DevicesTreeProvider } from "./views/devicesTree";
import { ProjectTreeProvider } from "./views/projectTree";
import { RunConfigsTreeProvider } from "./views/runConfigsTree";
import { SigningTreeProvider } from "./views/signingTree";
import { AgentActivityTreeProvider } from "./views/agentActivityTree";
import { StatusBar } from "./statusbar";
import { registerCommands } from "./commands";
import { RussetDebugConfigProvider } from "./debug/configProvider";
import { RussetDebugAdapterFactory } from "./debug/adapterFactory";
import { RussetDebugTrackerFactory } from "./debug/tracker";
import { registerIdeBridge } from "./debug/ideBridge";
import { installExperience } from "./setup";
import { applyMinimalDefaults } from "./setup/minimal";
import { registerAppleFiles } from "./apple";
import { registerAi } from "./ai";
import { log, disposeLog } from "./util/log";
import type { DeviceSummary } from "./generated/protocol";

export interface RussetApi {
  readonly apiVersion: 1;
  readonly client: Pick<RussetClient, "call" | "onEvent" | "state" | "onDidChangeState">;
  getActiveDevice(): DeviceSummary | undefined;
  readonly onDidChangeActiveDevice: vscode.Event<DeviceSummary | undefined>;
}

export function activate(context: vscode.ExtensionContext): RussetApi {
  const logger = log();
  logger.info(`Russet extension ${context.extension.packageJSON.version} activating`);
  void applyMinimalDefaults(context).catch((err: unknown) => logger.warn(`[setup] minimal defaults: ${String(err)}`));
  registerAppleFiles(context);

  const config = vscode.workspace.getConfiguration("russet");
  const backend = WindowsBackend.fromEnvironment();
  if (backend) context.subscriptions.push(backend);
  registerAi(context, backend?.home);
  const client = new RussetClient(
    {
      socketPath: config.get<string>("daemon.socketPath") || undefined,
      binDir: config.get<string>("binDir", ""),
      autoStart: config.get<boolean>("daemon.autoStart", true),
      clientVersion: String(context.extension.packageJSON.version),
      backend,
    },
    logger,
  );
  context.subscriptions.push(client);

  const store = new Store(context.workspaceState);
  context.subscriptions.push(store);

  const diagnostics = new Diagnostics();
  context.subscriptions.push(diagnostics);

  const consoleView = new ConsoleViewProvider(context.extensionUri);
  const mirrorView = new MirrorViewProvider(context.extensionUri, store);
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(ConsoleViewProvider.viewId, consoleView),
    vscode.window.registerWebviewViewProvider(MirrorViewProvider.viewId, mirrorView),
  );

  const run = new RunManager(client, store, consoleView, diagnostics);
  context.subscriptions.push(run);

  // Trees.
  const devicesTree = new DevicesTreeProvider(store);
  const projectTree = new ProjectTreeProvider(store);
  const runConfigsTree = new RunConfigsTreeProvider(store);
  const signingTree = new SigningTreeProvider(client);
  const agentActivityTree = new AgentActivityTreeProvider();
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("russet.devices", devicesTree),
    vscode.window.registerTreeDataProvider("russet.project", projectTree),
    vscode.window.registerTreeDataProvider("russet.runConfigs", runConfigsTree),
    vscode.window.registerTreeDataProvider("russet.signing", signingTree),
    vscode.window.registerTreeDataProvider("russet.agentActivity", agentActivityTree),
  );

  // Guided setup, Apple ID sign-in, run destinations and the Virtual iPhone tab.
  const experience = installExperience(context, { client, store, mirror: mirrorView, backendHome: backend?.home });
  context.subscriptions.push(devicesTree.useDestinations(experience.destinations));

  // Status bar.
  context.subscriptions.push(new StatusBar(store, client, experience.destinations));

  // Context keys and the daemon event stream.
  installContextKeys(store, client, context.subscriptions);
  installSubscriptions({ client, store, console: consoleView, diagnostics, run }, context.subscriptions);
  registerIdeBridge(client, context.subscriptions);

  // Debug type `russet`.
  const configProvider = new RussetDebugConfigProvider(client, store);
  const adapterFactory = new RussetDebugAdapterFactory(backend);
  context.subscriptions.push(adapterFactory, vscode.debug.onDidTerminateDebugSession((session) => adapterFactory.release(session.id)));
  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider("russet", configProvider),
    vscode.debug.registerDebugConfigurationProvider(
      "russet",
      configProvider,
      vscode.DebugConfigurationProviderTriggerKind.Dynamic,
    ),
    vscode.debug.registerDebugAdapterDescriptorFactory("russet", adapterFactory),
    vscode.debug.registerDebugAdapterDescriptorFactory("russet-virtual", adapterFactory),
    vscode.debug.registerDebugAdapterTrackerFactory("russet", new RussetDebugTrackerFactory(client, store)),
  );

  // Tasks.
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider(
      RussetTaskProvider.taskType,
      new RussetTaskProvider(client, store, diagnostics),
    ),
  );

  // Commands.
  registerCommands(context, {
    client,
    store,
    run,
    console: consoleView,
    mirror: mirrorView,
    signing: signingTree,
    refreshDevices: () => refreshDevices({ client, store, console: consoleView, diagnostics, run }),
    experience,
  });

  // Re-read the project when folders change.
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => void refreshProject(store, client)),
  );

  // Connect.
  void client.start();
  if (experience.runnerAvailable && !context.globalState.get<boolean>("windowsSetupShown", false)) {
    void vscode.commands.executeCommand("russet.openSetup").then(
      () => context.globalState.update("windowsSetupShown", true),
      () => logger.warn("Could not open the setup page."),
    );
  }

  const api: RussetApi = {
    apiVersion: 1,
    client: {
      call: client.call.bind(client),
      onEvent: client.onEvent.bind(client),
      get state() {
        return client.state;
      },
      onDidChangeState: client.onDidChangeState,
    } as RussetApi["client"],
    getActiveDevice: () => store.activeDevice,
    onDidChangeActiveDevice: store.onDidChangeActiveDevice,
  };
  return api;
}

export function deactivate(): void {
  disposeLog();
}

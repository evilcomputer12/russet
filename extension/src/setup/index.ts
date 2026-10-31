// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Wires the W19 IDE experience: the guided setup page over the Windows setup
// runner, the native Apple ID sign-in page, the Xcode-style run destination
// model (physical and virtual iPhones) and the Virtual iPhone editor tab.
// Walkthrough completion keys are set here from real state only.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import type { MirrorViewProvider } from "../panels/mirrorView";
import { DestinationModel, isVirtualDevice, pickDestination, type VirtualSource } from "../panels/destinations";
import { VirtualIphonePanel, type HardwareButton } from "../panels/virtualIphonePanel";
import { AppleIdApi, SignInController, type SignInMode } from "./appleId";
import { SetupModel, type DaemonProbe } from "./model";
import { SetupPanel } from "./setupPanel";
import { SignInPanel } from "./signInPanel";
import { SetupRunner } from "./runner";
import { downloadsDir, findXcodeXips } from "./xcode";
import { log } from "../util/log";

export interface Experience {
  readonly runnerAvailable: boolean;
  readonly setup: SetupModel;
  readonly destinations: DestinationModel;
  openSetup(step?: string): void;
  signIn(): void;
  importXcode(): void;
  selectDestination(): Promise<void>;
  /** Selects a daemon device (devices tree "Set as Run Destination"). */
  selectDevice(udid: string): void;
  /** Why Run/Debug cannot start on the selected destination, if it cannot. */
  runBlocker(): string | undefined;
}

const STATE_LABEL: Record<string, string> = {
  attached: "connecting",
  "needs-trust": "tap Trust on the iPhone",
  "trust-pending": "waiting for Trust",
  unavailable: "unavailable",
};

export function installExperience(
  context: vscode.ExtensionContext,
  deps: { client: RussetClient; store: Store; mirror: MirrorViewProvider },
): Experience {
  const { client, store } = deps;
  const runner = SetupRunner.fromEnvironment();
  const signInMode = (): SignInMode =>
    vscode.workspace.getConfiguration("russet").get<SignInMode>("account.appleIdSignIn", "auto");

  const probe: DaemonProbe = {
    connected: () => client.state === "connected",
    accounts: async () => {
      if (client.state !== "connected") {
        return undefined;
      }
      const res = await client.call("account.list", {});
      const def = res.accounts.find((a) => a.isDefault) ?? res.accounts[0];
      return { count: res.accounts.length, ...(def ? { label: def.label } : {}) };
    },
    toolchain: async () => {
      if (client.state !== "connected") {
        return undefined;
      }
      const res = await client.call("toolchain.list", {});
      return { swiftOk: res.tools.some((t) => t.component === "swift" && t.ok), sdkCount: res.sdks.length };
    },
    devices: () => {
      const phones = store.state.devices.filter((d) => !isVirtualDevice(d));
      const ready = phones.find((d) => d.state === "ready");
      const waiting = phones.find((d) => d.state !== "ready");
      return {
        ...(ready ? { ready: `${ready.marketingName ?? ready.productType}, iOS ${ready.osVersion}` } : {}),
        ...(waiting
          ? { waiting: `${waiting.marketingName ?? waiting.productType} is connected (${STATE_LABEL[waiting.state] ?? waiting.state})` }
          : {}),
      };
    },
  };

  const setup = new SetupModel({
    runner,
    daemon: probe,
    findXips: () => findXcodeXips(downloadsDir()),
    store: context.globalState,
    signInEnabled: () => signInMode() !== "off",
  });

  // Walkthrough completion keys (ide.md §5.8) from real state.
  const lastKeys = new Map<string, boolean>();
  const applyKeys = (): void => {
    for (const [key, value] of Object.entries(setup.contextKeys())) {
      if (lastKeys.get(key) !== value) {
        lastKeys.set(key, value);
        void vscode.commands.executeCommand("setContext", key, value);
      }
    }
  };
  const subs: Array<{ dispose(): void }> = [setup.onDidChange(applyKeys)];

  // Run destinations and the Virtual iPhone tab.
  const virtualSource = (): VirtualSource => ({
    created: setup.stepState("virtual-iphone") === "done",
    choice: setup.virtualChoice,
  });
  const destinations = new DestinationModel(store, context.workspaceState, virtualSource);
  context.subscriptions.push(destinations);
  let virtualSig = "";
  subs.push(
    setup.onDidChange(() => {
      const sig = JSON.stringify(virtualSource());
      if (sig !== virtualSig) {
        virtualSig = sig;
        destinations.refresh();
      }
    }),
  );
  deps.mirror.useDestinations(destinations);

  const pendingVirtualStep = (): string | undefined => {
    for (const id of ["vphone", "virtual-iphone"]) {
      const state = setup.stepState(id);
      if (state && state !== "done") {
        return setup.snapshot().steps.find((s) => s.id === id)?.title ?? id;
      }
    }
    return undefined;
  };
  const virtualPanel = new VirtualIphonePanel(context.extensionUri, destinations, () => {
    const pending = pendingVirtualStep();
    return { created: virtualSource().created, ...(pending ? { pendingStep: pending } : {}), choice: setup.virtualChoice };
  });
  const setupPanel = new SetupPanel(context.extensionUri, setup);

  // Native Apple ID sign-in.
  const signIn = new SignInController({
    api: new AppleIdApi((method, params) => client.call(method as never, params as never)),
    mode: signInMode,
    consentGiven: () => context.globalState.get<boolean>("russet.account.appleIdConsent", false),
    rememberConsent: () => context.globalState.update("russet.account.appleIdConsent", true),
  });
  const signInPanel = new SignInPanel(context.extensionUri, signIn);
  subs.push(
    signIn.onDidSignIn(() => {
      void setup.refresh();
      void vscode.commands.executeCommand("russet.signing.refresh");
    }),
  );

  context.subscriptions.push(setupPanel, signInPanel, virtualPanel, { dispose: () => subs.forEach((s) => s.dispose()) });

  // Restore the setup page and the Virtual iPhone tab after a window reload.
  context.subscriptions.push(
    vscode.window.registerWebviewPanelSerializer(SetupPanel.viewType, {
      deserializeWebviewPanel: async (panel) => setupPanel.attach(panel),
    }),
    vscode.window.registerWebviewPanelSerializer(VirtualIphonePanel.viewType, {
      deserializeWebviewPanel: async (panel) => virtualPanel.attach(panel),
    }),
  );

  // Commands this card adds (the pre-existing ids get their handlers through
  // registerCommands' `experience`).
  const reg = (id: string, fn: (...args: unknown[]) => unknown): void => {
    context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  };
  reg("russet.openVirtualIphone", () => virtualPanel.show());
  reg("russet.setup.openWalkthrough", () =>
    vscode.commands.executeCommand("workbench.action.openWalkthrough", "russet.russet#russet.setup", false),
  );
  for (const button of ["home", "lock", "volumeUp", "volumeDown"] as HardwareButton[]) {
    reg(`russet.virtual.${button}`, () => virtualPanel.button(button));
  }
  reg("russet.virtual.rotateLeft", () => virtualPanel.rotate("left"));
  reg("russet.virtual.rotateRight", () => virtualPanel.rotate("right"));
  reg("russet.virtual.screenshot", () => virtualPanel.screenshot());

  // Keep the model fresh: on connect, on device changes, on setting changes.
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  const refreshSoon = (): void => {
    if (refreshTimer) {
      clearTimeout(refreshTimer);
    }
    refreshTimer = setTimeout(() => {
      refreshTimer = undefined;
      setup.refresh().catch((err: unknown) => log().warn(`[setup] refresh failed: ${String(err)}`));
    }, 400);
  };
  context.subscriptions.push(
    client.onDidChangeState((s) => {
      if (s === "connected" || s === "stopped") {
        refreshSoon();
      }
    }),
    store.onDidChangeActiveDevice(() => setup.notify()),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("russet.account.appleIdSignIn")) {
        setup.notify();
      }
    }),
    { dispose: () => refreshTimer && clearTimeout(refreshTimer) },
  );
  refreshSoon();

  return {
    runnerAvailable: !!runner,
    setup,
    destinations,
    openSetup: (step) => setupPanel.show(step),
    signIn: () => {
      if (signInMode() === "off") {
        void vscode.window
          .showInformationMessage("Apple ID sign-in is turned off in Settings.", "Open Settings")
          .then((c) => c && vscode.commands.executeCommand("workbench.action.openSettings", "russet.account.appleIdSignIn"));
        return;
      }
      signInPanel.show();
    },
    importXcode: () => {
      if (runner) {
        setupPanel.show("xcode-sdk");
      } else {
        void vscode.window.showInformationMessage(
          "Importing Xcode from the IDE needs the engine's sdk.import (W16b); on Windows the guided setup does it.",
        );
      }
    },
    selectDestination: () => pickDestination(destinations, !!runner),
    selectDevice: (udid) => destinations.select(udid),
    runBlocker: () => destinations.runBlocker(),
  };
}

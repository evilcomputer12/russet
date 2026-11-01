// SPDX-License-Identifier: AGPL-3.0-only
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
import { virtualDebugConfiguration, virtualDebugSession, type VirtualDebugSession } from "./virtualDebug";
import { DestinationModel, isVirtualDevice, pickDestination, type VirtualSource } from "../panels/destinations";
import { VirtualIphonePanel, type HardwareButton } from "../panels/virtualIphonePanel";
import { AppleIdApi, SignInController, type SignInMode } from "./appleId";
import { SetupModel, type DaemonProbe } from "./model";
import { SetupPanel } from "./setupPanel";
import { SignInPanel } from "./signInPanel";
import { SetupRunner } from "./runner";
import { DEFAULT_CHECK_INTERVAL_SECONDS, UpdateWatcher } from "./updates";
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
  /** The selected destination is the setup-created virtual iPhone (runs go through the runner). */
  virtualSelected(): boolean;
  /** Builds the project and installs and launches it on the running virtual iPhone. */
  runOnVirtual(): Promise<void>;
  /** Builds, installs and launches it under the phone's debugserver, then attaches the debugger. */
  debugOnVirtual(): Promise<void>;
}

const STATE_LABEL: Record<string, string> = {
  attached: "connecting",
  "needs-trust": "tap Trust on the iPhone",
  "trust-pending": "waiting for Trust",
  unavailable: "unavailable",
};

export function installExperience(
  context: vscode.ExtensionContext,
  deps: { client: RussetClient; store: Store; mirror: MirrorViewProvider; backendHome?: string },
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

  // Run destinations and the Virtual iPhone tab. Whether the VM runs comes from the runner's
  // start and stop and its periodic update checks.
  let virtualRunning: boolean | undefined;
  const virtualSource = (): VirtualSource => ({
    created: setup.stepState("virtual-iphone") === "done",
    ...(virtualRunning !== undefined ? { running: virtualRunning } : {}),
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
  deps.mirror.useClient(deps.client);

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
    // Machine-scoped in the manifest; read only the user-settings value as well, so a workspace can never
    // redirect sign-in to another identity server.
    identityServer: () => {
      const i = vscode.workspace.getConfiguration("russet").inspect<string>("account.anisetteServer");
      return i?.globalValue ?? i?.defaultValue ?? "";
    },
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
  // Start and stop the runner-created virtual iPhone; its window opens on the desktop (WSLg).
  const virtualRun = async (id: "virtual-iphone-start" | "virtual-iphone-stop", title: string): Promise<void> => {
    if (!runner) {
      void vscode.window.showInformationMessage("Starting a virtual iPhone needs Russet's Windows backend.");
      return;
    }
    const choice = setup.virtualChoice;
    const args: Record<string, string> = id === "virtual-iphone-start" ? { device: choice.device, ios: choice.ios } : {};
    const outcome = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title },
      (progress) =>
        runner.run(id, args, (e) => {
          if (e.event === "progress") {
            progress.report({ message: e.message });
          }
          if (e.event === "progress" || e.event === "log") {
            log().info(`[${id}] ${e.message}`);
          }
        }),
    );
    if (outcome.kind === "done") {
      setVirtualRunning(id === "virtual-iphone-start");
      if (id === "virtual-iphone-start") {
        // Its screen and touch are in the Virtual iPhone tab.
        virtualPanel.show();
      }
    } else if (outcome.kind === "failed") {
      void vscode.window.showErrorMessage(outcome.message, "Show Log").then((c) => c && log().show());
    }
  };
  reg("russet.virtual.start", () => virtualRun("virtual-iphone-start", "Starting the virtual iPhone"));
  // Lock and volume as vphoned HID presses (the tab's toolbar and the side buttons).
  reg("russet.virtual.press", async (name: unknown) => {
    if (!runner || typeof name !== "string") {
      return;
    }
    const outcome = await runner.run("virtual-iphone-button", { name }, () => undefined);
    if (outcome.kind === "failed") {
      void vscode.window.showWarningMessage(outcome.message);
    }
  });
  // The lock screen ignores the emulated touchscreen's swipe and QEMU's Home shortcut: vphoned unlocks it.
  let unlocking = false;
  reg("russet.virtual.unlock", async () => {
    if (!runner || unlocking) {
      return;
    }
    unlocking = true;
    try {
      const outcome = await runner.run("virtual-iphone-unlock", {}, (e) => {
        if (e.event === "progress" || e.event === "log") {
          log().info(`[virtual-iphone-unlock] ${e.message}`);
        }
      });
      if (outcome.kind === "failed") {
        void vscode.window.showWarningMessage(outcome.message);
      }
    } finally {
      unlocking = false;
    }
  });
  reg("russet.virtual.stop", () => virtualRun("virtual-iphone-stop", "Stopping the virtual iPhone"));

  // WSL's memory limit (memory= in .wslconfig, shared by every WSL distro) and Russet's disk limit
  // (how far the backend's disk may grow before updates stop), both chosen by the owner here.
  reg("russet.wsl.limits", async () => {
    if (!runner) {
      void vscode.window.showInformationMessage("Changing WSL's limits needs Russet's Windows backend.");
      return;
    }
    let now;
    try {
      now = await runner.limits();
    } catch (err) {
      void vscode.window.showErrorMessage(`Russet could not read WSL's limits: ${(err as Error).message}`);
      return;
    }
    const gb = (bytes: number | undefined): number => Math.round((bytes ?? 0) / 2 ** 30);
    const machine = gb(now.machineMemoryBytes);
    const memoryNow = now.memoryGB ?? Math.round(machine / 2);
    const memory = await vscode.window.showInputBox({
      title: "WSL memory limit (1 of 2)",
      prompt:
        `GB of this PC's ${machine} GB that WSL may use, shared by every WSL distro; now ${memoryNow} GB` +
        `${now.memoryGB === undefined ? " (WSL's default)" : ""}${now.restartNeeded ? ", not yet applied" : ""}. The virtual iPhone needs about 16 GB.`,
      value: String(memoryNow),
      validateInput: (v) => (/^\d+$/.test(v) && +v >= 4 && +v <= machine ? undefined : `A whole number from 4 to ${machine}.`),
    });
    if (memory === undefined) {
      return;
    }
    const floor = Math.max(64, Math.ceil((now.diskBytes ?? 0) / 2 ** 30));
    const disk = await vscode.window.showInputBox({
      title: "Russet disk limit (2 of 2)",
      prompt:
        `GB Russet's backend disk may grow to; updates stop before they pass it. It takes ${gb(now.diskBytes)} GB now` +
        `${now.freeBytes === undefined ? "" : `, and its drive has ${gb(now.freeBytes)} GB free`}.`,
      value: String(now.diskGB),
      validateInput: (v) => (/^\d+$/.test(v) && +v >= floor && +v <= 4096 ? undefined : `A whole number from ${floor} to 4096.`),
    });
    if (disk === undefined) {
      return;
    }
    const args: Record<string, string> = {};
    if (+memory !== now.memoryGB) {
      args.memoryGB = memory;
    }
    if (+disk !== now.diskGB) {
      args.diskGB = disk;
    }
    if (Object.keys(args).length === 0) {
      void vscode.window.showInformationMessage("WSL's limits are unchanged.");
      return;
    }
    if (args.memoryGB) {
      const pick = await vscode.window.showWarningMessage(
        `Set WSL's memory limit to ${memory} GB?`,
        {
          modal: true,
          detail:
            "Russet sets memory= under [wsl2] in %USERPROFILE%\\.wslconfig, which every WSL distro shares. The rest of the file stays as it is, " +
            "and Russet keeps a copy of the file from before its first change (%LOCALAPPDATA%\\Russet\\wslconfig.before-russet).",
        },
        "Set Limit",
      );
      if (pick !== "Set Limit") {
        return;
      }
    }
    const outcome = await runner.run("wsl-limits", args, (e) => {
      if (e.event === "progress" || e.event === "log") {
        log().info(`[wsl-limits] ${e.message}`);
      }
    });
    if (outcome.kind === "failed") {
      void vscode.window.showErrorMessage(outcome.message);
      return;
    }
    if (!args.memoryGB) {
      void vscode.window.showInformationMessage(`Russet's disk limit is now ${disk} GB.`);
      return;
    }
    const restart = await vscode.window.showInformationMessage(
      `WSL uses ${memory} GB once it restarts.`,
      {
        modal: true,
        detail:
          "Restarting now stops the virtual iPhone, closes Russet, shuts down every WSL distro and reopens Russet. " +
          "Otherwise the limit applies the next time WSL starts, such as after Windows restarts.",
      },
      "Restart WSL and Russet",
    );
    if (restart !== "Restart WSL and Russet") {
      return;
    }
    if (virtualRunning) {
      await virtualRun("virtual-iphone-stop", "Stopping the virtual iPhone");
    }
    const restarted = await runner.run("wsl-restart", {}, () => undefined);
    if (restarted.kind === "done") {
      await vscode.commands.executeCommand("workbench.action.quit");
    } else if (restarted.kind === "failed") {
      void vscode.window.showErrorMessage(restarted.message);
    }
  });

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
      // A changed identity server applies to the next sign-in; restart a page that has not signed in yet.
      const screen = signIn.view.screen;
      if (
        (e.affectsConfiguration("russet.account.anisetteServer") || e.affectsConfiguration("russet.account.appleIdSignIn")) &&
        (screen === "unavailable" || screen === "consent" || screen === "credentials")
      ) {
        void signIn.start();
      }
    }),
    { dispose: () => refreshTimer && clearTimeout(refreshTimer) },
  );
  refreshSoon();

  // Steps Russet requires before anything else: while one is needed, every window opens the
  // setup page on it (once per window) and says why.
  let requiredShown = false;
  const REQUIRED: Record<string, string> = {
    wsl: "Russet needs the Windows Subsystem for Linux. Install it on the setup page (administrator rights; Windows may restart).",
    backend: "Russet's backend needs installing, or its engine in WSL updating to this release's. Run the Russet backend step on the setup page (a first install downloads 0.4 GB of Ubuntu).",
    "wsl-update": "Russet requires WSL's pre-release channel for GPU rendering of the virtual iPhone. Run the WSL pre-release step on the setup page.",
  };
  subs.push(
    setup.onDidChange(() => {
      // A backend the setup page just installed has its daemon running: connect to it.
      if (setup.stepState("backend") === "done" && client.state === "stopped") {
        void client.start();
      }
      const required = Object.keys(REQUIRED).find((id) => setup.stepState(id) === "needed");
      if (requiredShown || !required) {
        return;
      }
      requiredShown = true;
      setupPanel.show(required);
      void vscode.window
        .showWarningMessage(REQUIRED[required], "Open Setup")
        .then((pick) => pick && setupPanel.show(required));
    }),
  );

  // Upstream updates of the managed components (the virtual iPhone engine), built beside the
  // build in use and switched to when idle.
  const setVirtualRunning = (running: boolean): void => {
    virtualPanel.setRunning(running);
    if (virtualRunning !== running) {
      virtualRunning = running;
      destinations.refresh();
    }
  };
  if (runner) {
    const updates = new UpdateWatcher(
      runner,
      {
        info: (message, ...actions) => Promise.resolve(vscode.window.showInformationMessage(message, ...actions)),
        warn: (message, ...actions) => Promise.resolve(vscode.window.showWarningMessage(message, ...actions)),
        log: (line) => log().info(`[updates] ${line}`),
        changeLimits: () => Promise.resolve(vscode.commands.executeCommand("russet.wsl.limits")),
      },
      (components) => {
        const engine = components.find((c) => c.id === "vphone");
        if (engine) {
          setVirtualRunning(engine.running);
        }
      },
    );
    const interval = (): number =>
      vscode.workspace.getConfiguration("russet").get<number>("updates.checkIntervalSeconds", DEFAULT_CHECK_INTERVAL_SECONDS);
    updates.schedule(interval());
    context.subscriptions.push(
      updates,
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("russet.updates.checkIntervalSeconds")) {
          updates.schedule(interval());
        }
      }),
    );
  }

  // Run and Debug on the virtual iPhone: build, then install (and launch) through the runner, which
  // vphoned signs; Debug launches it under the phone's debugserver and attaches lldb-dap.
  const onVirtual = async (debug: boolean): Promise<void> => {
    if (!runner) {
      return;
    }
    const project = store.state.project;
    const root = project?.root ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root || client.state !== "connected") {
      void vscode.window.showWarningMessage("Open a project, and wait for Russet's backend to connect.");
      return;
    }
    const step = debug ? "virtual-iphone-debug" : "virtual-iphone-install";
    let session: VirtualDebugSession | undefined;
    await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: debug ? "Debugging on the virtual iPhone" : "Running on the virtual iPhone" },
      async (progress) => {
        try {
          progress.report({ message: "Building…" });
          const output = await client.call("build.start", {
            project: root,
            ...(store.state.selectedTarget ? { target: store.state.selectedTarget } : {}),
          });
          progress.report({ message: `Installing ${output.app.bundleId}…` });
          // Debug sockets go in the backend's runtime folder, the only place its relay opens.
          const args: Record<string, string> = { path: output.app.path };
          if (debug && deps.backendHome) {
            args.runtime = `${deps.backendHome}/runtime`;
          }
          const outcome = await runner.run(step, args, (e) => {
            if (e.event === "progress") {
              progress.report({ message: e.message });
            }
            if (e.event === "progress" || e.event === "log") {
              log().info(`[${step}] ${e.message}`);
            }
          });
          if (outcome.kind === "failed") {
            void vscode.window.showErrorMessage(outcome.message, "Show Log").then((c) => c && log().show());
            return;
          }
          virtualPanel.show();
          if (debug) {
            session = virtualDebugSession(outcome.kind === "done" ? outcome.result : undefined);
            if (!session) {
              void vscode.window
                .showErrorMessage("The virtual iPhone's debugger did not report its session.", "Show Log")
                .then((c) => c && log().show());
            }
          }
        } catch (err) {
          void vscode.window.showErrorMessage(`${debug ? "Debugging" : "Running"} on the virtual iPhone failed: ${(err as Error).message}`);
        }
      },
    );
    if (session) {
      log().info(`[${step}] attached to pid ${session.pid}; starting lldb-dap`);
      const folder = vscode.workspace.workspaceFolders?.[0];
      const started = await vscode.debug.startDebugging(folder, virtualDebugConfiguration(session));
      log().info(`[${step}] debug session ${started ? "started" : "did not start"}`);
      if (!started) {
        void vscode.window.showErrorMessage("Could not start the debug session on the virtual iPhone.");
      }
    }
  };

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
    virtualSelected: () => {
      const d = destinations.selected;
      return d?.kind === "virtual" && !d.device;
    },
    runOnVirtual: () => onVirtual(false),
    debugOnVirtual: () => onVirtual(true),
  };
}

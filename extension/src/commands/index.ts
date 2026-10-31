// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Registers every command id declared in the manifest (ide.md §5.6). Tier-0
// features are wired; later-card features register a graceful placeholder so the
// contribution manifest stays complete and no command id is missing.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Store } from "../state/store";
import type { RunManager } from "../run/session";
import type { ConsoleViewProvider } from "../panels/consoleView";
import type { MirrorViewProvider } from "../panels/mirrorView";
import type { SigningTreeProvider } from "../views/signingTree";
import type { Experience } from "../setup";
import { pickDevice, pickTarget, pickConfiguration } from "./pickers";
import { hotReload } from "./hotReload";
import { showDaemonError } from "../daemon/errors";
import { log } from "../util/log";

export interface CommandServices {
  client: RussetClient;
  store: Store;
  run: RunManager;
  console: ConsoleViewProvider;
  mirror: MirrorViewProvider;
  signing: SigningTreeProvider;
  refreshDevices: () => Promise<void>;
  /** The W19 setup, sign-in and run-destination surfaces. */
  experience?: Experience;
}

export function registerCommands(context: vscode.ExtensionContext, svc: CommandServices): void {
  const reg = (id: string, fn: (...args: unknown[]) => unknown): void => {
    context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  };
  const soon = (feature: string): void => {
    void vscode.window.showInformationMessage(`Russet: ${feature} is coming in a later update.`);
  };

  // --- Run / Debug / Stop / Hot reload -------------------------------------
  const blockedRun = (): boolean => {
    const reason = svc.experience?.runBlocker();
    if (reason) {
      void vscode.window.showInformationMessage(reason, "Select Run Destination").then((c) =>
        c ? vscode.commands.executeCommand("russet.selectDevice") : undefined,
      );
    }
    return !!reason;
  };
  reg("russet.run", () => (blockedRun() ? undefined : svc.run.run()));
  reg("russet.debug", async () => {
    if (blockedRun()) {
      return;
    }
    const folder = vscode.workspace.workspaceFolders?.[0];
    const ok = await vscode.debug.startDebugging(folder, {
      type: "russet",
      request: "launch",
      name: "Debug on iPhone",
      target: svc.store.state.selectedTarget,
      device: "auto",
      configuration: svc.store.state.selectedConfiguration,
      stopAtEntry: false,
      build: true,
    });
    if (!ok) {
      void vscode.window.showErrorMessage("Could not start the debug session.");
    }
  });
  reg("russet.stop", async () => {
    const debugSession = vscode.debug.activeDebugSession;
    if (debugSession && debugSession.type === "russet") {
      await vscode.debug.stopDebugging(debugSession);
      return;
    }
    await svc.run.stop();
  });
  reg("russet.hotReload", () => hotReload(svc.client, svc.store, "reload"));
  reg("russet.hotRestart", () => hotReload(svc.client, svc.store, "restart"));

  // --- Build tasks ----------------------------------------------------------
  reg("russet.build", () => runRussetTask("build"));
  reg("russet.clean", () => runRussetTask("clean"));

  // --- Pickers --------------------------------------------------------------
  reg("russet.selectDevice", () => (svc.experience ? svc.experience.selectDestination() : pickDevice(svc.store)));
  reg("russet.selectTarget", () => pickTarget(svc.store));
  reg("russet.selectConfiguration", () => pickConfiguration(svc.store));
  reg("russet.device.setRunDestination", (node: unknown) => {
    const udid = extractUdid(node) ?? svc.store.activeDevice?.udid;
    if (udid && svc.experience) {
      svc.experience.selectDevice(udid);
    } else if (udid) {
      svc.store.setActiveDevice(udid);
    } else {
      void vscode.commands.executeCommand("russet.selectDevice");
    }
  });

  // --- Devices --------------------------------------------------------------
  reg("russet.devices.refresh", () => svc.refreshDevices());
  reg("russet.device.copyUdid", async (node: unknown) => {
    const udid = extractUdid(node) ?? svc.store.activeDevice?.udid;
    if (udid) {
      await vscode.env.clipboard.writeText(udid);
      void vscode.window.showInformationMessage("UDID copied to the clipboard.");
    }
  });
  reg("russet.device.pair", () => daemonDeviceAction("device.pair", "Pairing"));
  reg("russet.device.mountDdi", () => daemonDeviceAction("device.ddi.mount", "Mounting the Developer Disk Image"));
  reg("russet.device.revealDeveloperMode", () =>
    daemonDeviceAction("device.developerMode.reveal" as never, "Revealing Developer Mode"),
  );
  reg("russet.device.installApp", () => soon("Install .ipa/.app"));

  // --- Console / Mirror / Screenshot ---------------------------------------
  reg("russet.openConsole", () => svc.console.reveal(svc.console.currentPid));
  reg("russet.openMirror", () => svc.mirror.reveal());
  reg("russet.takeScreenshot", () => soon("Take Screenshot"));

  // --- Doctor / setup -------------------------------------------------------
  reg("russet.doctor", runDoctor);
  reg("russet.openSetup", async (step?: unknown) => {
    if (svc.experience) {
      svc.experience.openSetup(typeof step === "string" ? step : undefined);
      return;
    }
    await vscode.commands.executeCommand(
      "workbench.action.openWalkthrough",
      "russet.russet#russet.setup",
      false,
    );
  });
  reg("russet.importXcode", () => (svc.experience ? svc.experience.importXcode() : soon("Import Xcode")));

  // --- Project --------------------------------------------------------------
  reg("russet.project.new", async () => {
    const name = await vscode.window.showInputBox({
      prompt: "New app name",
      placeHolder: "My Great App",
      validateInput: (v) => (v.trim().length === 0 ? "Enter a name" : undefined),
    });
    if (!name) {
      return;
    }
    const picked = await vscode.window.showOpenDialog({
      canSelectFolders: true,
      canSelectFiles: false,
      canSelectMany: false,
      openLabel: "Create here",
      title: `Choose where to create "${name.trim()}"`,
    });
    if (!picked || picked.length === 0) {
      return;
    }
    const root = vscode.Uri.joinPath(picked[0], name.trim());
    try {
      const res = (await svc.client.call("project.init" as never, {
        dir: root.fsPath,
        stack: "swiftpm",
        name: name.trim(),
        force: false,
      } as never)) as { root: string; files: string[] };
      const open = await vscode.window.showInformationMessage(
        `Created ${name.trim()} (${res.files.length} files).`,
        "Open",
        "Open in New Window",
      );
      if (open === "Open") {
        await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(res.root));
      } else if (open === "Open in New Window") {
        await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(res.root), {
          forceNewWindow: true,
        });
      }
    } catch (err) {
      await showDaemonError(err, `Could not create ${name.trim()}`);
    }
  });

  // --- Accounts / signing ---------------------------------------------------
  reg("russet.account.addAscKey", addAscKey);
  reg("russet.account.signInAppleId", () => (svc.experience ? svc.experience.signIn() : soon("Sign in with Apple ID")));
  reg("russet.account.signOut", () => soon("Sign Out"));
  reg("russet.signing.refresh", () => svc.signing.refresh());
  reg("russet.signing.fix", () => soon("Fix Signing"));

  // --- Daemon ---------------------------------------------------------------
  reg("russet.daemon.start", () => svc.client.start());
  reg("russet.daemon.restart", async () => {
    try {
      await svc.client.call("daemon.shutdown" as never, { reason: "restart from IDE" } as never);
    } catch {
      /* ignore */
    }
    await svc.client.start();
  });
  reg("russet.daemon.showLog", () => log().show());

  // --- Editors / panels (later cards) --------------------------------------
  reg("russet.openInfoPlist", () => openOwnedFile("Info.plist"));
  reg("russet.openEntitlements", () => openOwnedFile(".entitlements"));
  reg("russet.openCapabilities", () => vscode.commands.executeCommand("russet.signing.focus"));
  reg("russet.openAssetCatalog", () => soon("Asset catalog editor"));
  reg("russet.newAppIcon", () => soon("App icon editor"));
  reg("russet.openPublishing", () => soon("Publishing panel"));
  reg("russet.ship.upload", () => soon("Upload to TestFlight"));
  reg("russet.openProfiler", () => soon("Profiler"));
  reg("russet.openCrashes", () => soon("Crash viewer"));

  // --- Agents / MCP (later cards) ------------------------------------------
  reg("russet.agents.open", () => soon("Agents panel"));
  reg("russet.agents.newSession", () => soon("Agent sessions"));
  reg("russet.agents.install", () => soon("Agent install"));
  reg("russet.agents.quickAsk", () => soon("Ask Local Model"));
  reg("russet.mcp.register", () => soon("Register Russet MCP"));
  reg("russet.mcp.status", () => soon("MCP status"));
  reg("russet.mcp.copyConfig", () => soon("Copy MCP config"));
  reg("russet.mcp.toggleHttp", () => soon("MCP HTTP endpoint"));

  // --- Shell ----------------------------------------------------------------
  reg("russet.shell.install", () => soon("Shell command install"));

  // --- Helpers --------------------------------------------------------------
  async function runRussetTask(command: "build" | "clean"): Promise<void> {
    const tasks = await vscode.tasks.fetchTasks({ type: "russet" });
    const task = tasks.find((t) => (t.definition as { command?: string }).command === command);
    if (task) {
      await vscode.tasks.executeTask(task);
    } else {
      void vscode.window.showErrorMessage(`No Russet ${command} task is available (open a project).`);
    }
  }

  async function daemonDeviceAction(method: string, label: string): Promise<void> {
    const udid = svc.store.activeDevice?.udid;
    try {
      await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: label },
        () => svc.client.call(method as never, { udid } as never),
      );
      await svc.refreshDevices();
    } catch (err) {
      await showDaemonError(err, label);
    }
  }

  async function runDoctor(): Promise<void> {
    try {
      const report = await svc.client.call("doctor.run", {});
      const checks = report.checks;
      const failed = checks.filter((c) => c.status === "fail" || c.status === "warn");
      log().show();
      log().info("[doctor] results:");
      for (const c of checks) {
        log().info(`  [${c.status}] ${c.title}${c.detail ? ` — ${c.detail}` : ""}`);
      }
      if (failed.length === 0) {
        void vscode.commands.executeCommand("setContext", "russet.doctorOk", true);
        void vscode.window.showInformationMessage("Russet doctor: all checks passed.");
      } else {
        void vscode.window.showWarningMessage(
          `Russet doctor: ${failed.length} check(s) need attention. See the Russet output.`,
        );
      }
    } catch (err) {
      await showDaemonError(err, "Doctor failed");
    }
  }

  async function addAscKey(): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      title: "Select an App Store Connect API key",
      filters: { "API key": ["p8"] },
      canSelectMany: false,
    });
    if (!picked || picked.length === 0) {
      return;
    }
    const p8Path = picked[0].fsPath;
    const m = /AuthKey_([A-Z0-9]+)\.p8$/i.exec(p8Path);
    const keyId = m?.[1];
    if (!keyId) {
      void vscode.window.showErrorMessage("The file name must be AuthKey_<KEYID>.p8.");
      return;
    }
    const issuerId = await vscode.window.showInputBox({
      title: "Issuer ID",
      prompt: "The App Store Connect issuer ID (omit for an individual key).",
    });
    try {
      await svc.client.call("account.addAscKey", {
        keyId,
        issuerId: issuerId || undefined,
        p8Path,
      });
      await svc.signing.refresh();
      void vscode.window.showInformationMessage(`Added API key ${keyId}.`);
    } catch (err) {
      await showDaemonError(err, "Could not add the API key");
    }
  }

  async function openOwnedFile(suffix: string): Promise<void> {
    const found = await vscode.workspace.findFiles(`**/*${suffix}`, "**/node_modules/**", 1);
    if (found.length > 0) {
      await vscode.window.showTextDocument(found[0]);
    } else {
      void vscode.window.showInformationMessage(`No ${suffix} file found in this workspace.`);
    }
  }
}

function extractUdid(node: unknown): string | undefined {
  if (node && typeof node === "object") {
    const n = node as { device?: { udid?: string }; udid?: string };
    return n.device?.udid ?? n.udid;
  }
  return undefined;
}

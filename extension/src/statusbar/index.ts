// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The status bar row (ide.md §5.6): Xcode-style Run / Debug / Stop, the run
// destination picker (physical and virtual iPhones), target/configuration
// picker, build state and account. Left to right, highest priority first.

import * as vscode from "vscode";
import type { Store } from "../state/store";
import type { RussetClient } from "../daemon/client";
import type { DestinationModel } from "../panels/destinations";

export class StatusBar implements vscode.Disposable {
  private readonly runItem: vscode.StatusBarItem;
  private readonly debugItem: vscode.StatusBarItem;
  private readonly stopItem: vscode.StatusBarItem;
  private readonly device: vscode.StatusBarItem;
  private readonly target: vscode.StatusBarItem;
  private readonly build: vscode.StatusBarItem;
  private readonly account: vscode.StatusBarItem;
  private readonly disposables: vscode.Disposable[] = [];
  private accountLabel = "";

  constructor(
    private readonly store: Store,
    private readonly client: RussetClient,
    private readonly destinations?: DestinationModel,
  ) {
    const left = vscode.StatusBarAlignment.Left;
    this.runItem = vscode.window.createStatusBarItem("russet.runControl", left, 103);
    this.debugItem = vscode.window.createStatusBarItem("russet.debugControl", left, 102);
    this.stopItem = vscode.window.createStatusBarItem("russet.stopControl", left, 101);
    for (const [item, name] of [
      [this.runItem, "Russet: Run"],
      [this.debugItem, "Russet: Debug"],
      [this.stopItem, "Russet: Stop"],
    ] as const) {
      item.name = name;
    }
    this.runItem.command = "russet.run";
    this.debugItem.command = "russet.debug";
    this.stopItem.command = "russet.stop";
    this.device = vscode.window.createStatusBarItem("russet.device", left, 100);
    this.target = vscode.window.createStatusBarItem("russet.target", left, 99);
    this.build = vscode.window.createStatusBarItem("russet.build", left, 98);
    this.account = vscode.window.createStatusBarItem("russet.account", left, 97);

    this.device.command = "russet.selectDevice";
    this.target.command = "russet.selectTarget";
    this.account.command = "russet.openCapabilities";

    this.disposables.push(this.runItem, this.debugItem, this.stopItem, this.device, this.target, this.build, this.account);
    if (destinations) {
      this.disposables.push(destinations.onDidChange(() => this.render()));
    }
    this.disposables.push(store.onDidChange(() => this.render()));
    this.disposables.push(
      client.onDidChangeState((s) => {
        if (s === "connected") {
          void this.loadAccount();
        }
        this.render();
      }),
    );
    void this.loadAccount();
    this.render();
  }

  private async loadAccount(): Promise<void> {
    try {
      const result = await this.client.call("account.list", {});
      const def = result.accounts.find((a) => a.isDefault) ?? result.accounts[0];
      this.accountLabel = def?.teamId ?? def?.label ?? "";
    } catch {
      this.accountLabel = "";
    }
    this.render();
  }

  private render(): void {
    const connected = this.client.state === "connected";
    const s = this.store.state;

    this.renderRunControls(connected);

    // Device (run destination)
    const dev = this.store.activeDevice;
    const dest = this.destinations?.selected;
    if (connected && dest?.kind === "virtual") {
      this.device.text = `$(vm) ${dest.name} · iOS ${dest.osVersion ?? "?"}`;
      this.device.tooltip = `Run destination: virtual ${dest.name} (${dest.stateLabel}). Click to change.`;
      this.device.backgroundColor = undefined;
      this.device.accessibilityInformation = {
        label: `Run destination: virtual ${dest.name}, iOS ${dest.osVersion ?? "unknown"}, ${dest.stateLabel}. Change run destination.`,
        role: "button",
      };
    } else if (!connected) {
      this.device.text = "$(circle-slash) Russet: offline";
      this.device.accessibilityInformation = undefined;
      this.device.tooltip = "The Russet daemon is not connected.";
      this.device.backgroundColor = new vscode.ThemeColor("statusBarItem.warningBackground");
    } else if (dev) {
      this.device.text = `$(device-mobile) ${dev.marketingName ?? dev.name} · iOS ${dev.osVersion}`;
      this.device.accessibilityInformation = {
        label: `Run destination: ${dev.marketingName ?? dev.name}, iOS ${dev.osVersion}, ${dev.state}. Change run destination.`,
        role: "button",
      };
      this.device.tooltip = `Run destination: ${dev.name} (${dev.state}). Click to change.`;
      this.device.backgroundColor =
        dev.state === "ready"
          ? undefined
          : new vscode.ThemeColor("statusBarItem.warningBackground");
    } else {
      this.device.text = "$(device-mobile) No device";
      this.device.accessibilityInformation = undefined;
      this.device.tooltip = "No iPhone selected. Click to choose a run destination.";
      this.device.backgroundColor = undefined;
    }
    this.device.show();

    // Target · configuration
    if (s.project) {
      const t = s.selectedTarget ?? s.project.name;
      this.target.text = `$(target) ${t} · ${s.selectedConfiguration}`;
      this.target.tooltip = "Target and build configuration. Click to change.";
      this.target.show();
    } else {
      this.target.hide();
    }

    // Build state
    switch (s.build.phase) {
      case "building":
        this.build.text = "$(sync~spin) Building…";
        this.build.backgroundColor = undefined;
        this.build.show();
        break;
      case "ok":
        this.build.text = `$(check) ${((s.build.durationMs ?? 0) / 1000).toFixed(1)} s`;
        this.build.backgroundColor = undefined;
        this.build.show();
        break;
      case "error":
        this.build.text = `$(error) ${s.build.errorCount ?? 0}`;
        this.build.backgroundColor = new vscode.ThemeColor("statusBarItem.errorBackground");
        this.build.show();
        break;
      default:
        this.build.hide();
    }

    // Account
    if (connected && this.accountLabel) {
      this.account.text = `$(account) ${this.accountLabel}`;
      this.account.tooltip = "Signing account. Click to open the Signing view.";
      this.account.show();
    } else {
      this.account.hide();
    }
  }

  /** Xcode-style Run / Debug / Stop, shown while a project is open. */
  private renderRunControls(connected: boolean): void {
    const s = this.store.state;
    const running =
      s.run.state !== "idle" && s.run.state !== "exited" && s.run.state !== "crashed" &&
      s.run.state !== "failed" && s.run.state !== "lost";
    const where = this.destinations?.selected?.name ?? this.store.activeDevice?.marketingName ?? "the iPhone";
    if (!s.project || !connected) {
      this.runItem.hide();
      this.debugItem.hide();
      this.stopItem.hide();
      return;
    }
    const building = s.build.phase === "building";
    this.runItem.text = building ? "$(sync~spin)" : "$(play)";
    this.runItem.tooltip = running ? `Run again on ${where} (Ctrl+F5)` : `Run on ${where} (Ctrl+F5)`;
    this.runItem.accessibilityInformation = { label: `Run on ${where}`, role: "button" };
    this.debugItem.text = "$(debug-alt)";
    this.debugItem.tooltip = `Debug on ${where}`;
    this.debugItem.accessibilityInformation = { label: `Debug on ${where}`, role: "button" };
    this.stopItem.text = "$(debug-stop)";
    this.stopItem.tooltip = "Stop (Shift+F5)";
    this.stopItem.accessibilityInformation = { label: "Stop the running app", role: "button" };
    this.runItem.show();
    this.debugItem.show();
    if (running || building) {
      this.stopItem.show();
    } else {
      this.stopItem.hide();
    }
  }

  dispose(): void {
    for (const d of this.disposables) {
      d.dispose();
    }
  }
}

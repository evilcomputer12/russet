// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The status bar row (ide.md §5.6): device picker, target/configuration picker,
// build state and account. Left to right, highest priority first.

import * as vscode from "vscode";
import type { Store } from "../state/store";
import type { RussetClient } from "../daemon/client";

export class StatusBar implements vscode.Disposable {
  private readonly device: vscode.StatusBarItem;
  private readonly target: vscode.StatusBarItem;
  private readonly build: vscode.StatusBarItem;
  private readonly account: vscode.StatusBarItem;
  private readonly disposables: vscode.Disposable[] = [];
  private accountLabel = "";

  constructor(
    private readonly store: Store,
    private readonly client: RussetClient,
  ) {
    const left = vscode.StatusBarAlignment.Left;
    this.device = vscode.window.createStatusBarItem("russet.device", left, 100);
    this.target = vscode.window.createStatusBarItem("russet.target", left, 99);
    this.build = vscode.window.createStatusBarItem("russet.build", left, 98);
    this.account = vscode.window.createStatusBarItem("russet.account", left, 97);

    this.device.command = "russet.selectDevice";
    this.target.command = "russet.selectTarget";
    this.account.command = "russet.openCapabilities";

    this.disposables.push(this.device, this.target, this.build, this.account);
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

    // Device
    const dev = this.store.activeDevice;
    if (!connected) {
      this.device.text = "$(circle-slash) Russet: offline";
      this.device.tooltip = "The Russet daemon is not connected.";
      this.device.backgroundColor = new vscode.ThemeColor("statusBarItem.warningBackground");
    } else if (dev) {
      this.device.text = `$(device-mobile) ${dev.marketingName ?? dev.name} · iOS ${dev.osVersion}`;
      this.device.tooltip = `Run destination: ${dev.name} (${dev.state}). Click to change.`;
      this.device.backgroundColor =
        dev.state === "ready"
          ? undefined
          : new vscode.ThemeColor("statusBarItem.warningBackground");
    } else {
      this.device.text = "$(device-mobile) No device";
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

  dispose(): void {
    for (const d of this.disposables) {
      d.dispose();
    }
  }
}

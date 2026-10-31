// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Devices view (ide.md §5.6, EXT-03): each device with its state icon and a
// USB/Wi-Fi badge; children open the console, mirror and crash reports.

import * as vscode from "vscode";
import type { Store } from "../state/store";
import type { DeviceSummary, DeviceState } from "../generated/protocol";

type Node = DeviceNode | ActionNode;

interface DeviceNode {
  kind: "device";
  device: DeviceSummary;
}

interface ActionNode {
  kind: "action";
  udid: string;
  label: string;
  icon: string;
  command: string;
}

const STATE_ICON: Record<DeviceState, { icon: string; color?: string }> = {
  ready: { icon: "pass-filled", color: "testing.iconPassed" },
  attached: { icon: "plug", color: "charts.yellow" },
  "needs-trust": { icon: "shield", color: "charts.yellow" },
  "trust-pending": { icon: "loading~spin" },
  unavailable: { icon: "error", color: "errorForeground" },
};

export class DevicesTreeProvider implements vscode.TreeDataProvider<Node> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  constructor(private readonly store: Store) {
    store.onDidChange(() => this._onDidChangeTreeData.fire());
  }

  getTreeItem(node: Node): vscode.TreeItem {
    if (node.kind === "action") {
      const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.None);
      item.iconPath = new vscode.ThemeIcon(node.icon);
      item.command = { command: node.command, title: node.label };
      item.contextValue = "russet.deviceChild";
      return item;
    }
    const d = node.device;
    const name = d.marketingName ?? d.name;
    const active = this.store.state.activeDeviceUdid === d.udid;
    const item = new vscode.TreeItem(name, vscode.TreeItemCollapsibleState.Collapsed);
    const badge = d.transport === "wifi" ? "$(radio-tower) Wi-Fi" : "$(plug) USB";
    item.description = `${d.productType} · iOS ${d.osVersion} · ${badge}`;
    const st = STATE_ICON[d.state] ?? { icon: "question" };
    item.iconPath = new vscode.ThemeIcon(
      st.icon,
      st.color ? new vscode.ThemeColor(st.color) : undefined,
    );
    item.contextValue = "russet.device";
    item.tooltip = new vscode.MarkdownString(
      `**${name}**\n\n- State: \`${d.state}\`${d.error ? ` (${d.error.russetCode})` : ""}\n- Build: ${d.build ?? "?"}\n- Monitor: ${d.codeSigningMonitor ?? "unknown"}`,
    );
    if (active) {
      item.description = `✓ ${item.description}`;
    }
    return item;
  }

  getChildren(node?: Node): Node[] {
    if (!node) {
      return this.store.state.devices.map((device) => ({ kind: "device", device }));
    }
    if (node.kind === "device") {
      const udid = node.device.udid;
      return [
        { kind: "action", udid, label: "Open Console", icon: "terminal", command: "russet.openConsole" },
        { kind: "action", udid, label: "Mirror Screen", icon: "device-mobile", command: "russet.openMirror" },
        { kind: "action", udid, label: "Crash Reports", icon: "bug", command: "russet.openCrashes" },
      ];
    }
    return [];
  }
}

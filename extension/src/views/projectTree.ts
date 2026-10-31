// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Project view (ide.md §5.6, EXT-04): targets with Info.plist, Entitlements,
// Assets and Signing & Capabilities children, plus Configurations.

import * as vscode from "vscode";
import type { Store } from "../state/store";
import type { ProjectTarget } from "../generated/protocol";

type Node =
  | { kind: "target"; target: ProjectTarget }
  | { kind: "editor"; label: string; icon: string; command: string }
  | { kind: "group"; label: string; items: string[] };

export class ProjectTreeProvider implements vscode.TreeDataProvider<Node> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  constructor(private readonly store: Store) {
    store.onDidChange(() => this._onDidChangeTreeData.fire());
  }

  getTreeItem(node: Node): vscode.TreeItem {
    if (node.kind === "target") {
      const item = new vscode.TreeItem(node.target.name, vscode.TreeItemCollapsibleState.Expanded);
      item.description = node.target.kind + (node.target.bundleId ? ` · ${node.target.bundleId}` : "");
      item.iconPath = new vscode.ThemeIcon("package");
      item.contextValue = "russet.target";
      return item;
    }
    if (node.kind === "group") {
      const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.Collapsed);
      item.iconPath = new vscode.ThemeIcon("list-tree");
      return item;
    }
    const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.None);
    item.iconPath = new vscode.ThemeIcon(node.icon);
    item.command = { command: node.command, title: node.label };
    return item;
  }

  getChildren(node?: Node): Node[] {
    const project = this.store.state.project;
    if (!project) {
      return [];
    }
    if (!node) {
      const nodes: Node[] = project.targets.map((target) => ({ kind: "target", target }));
      nodes.push({ kind: "group", label: "Configurations", items: project.configurations });
      return nodes;
    }
    if (node.kind === "target") {
      return [
        { kind: "editor", label: "Info.plist", icon: "list-flat", command: "russet.openInfoPlist" },
        { kind: "editor", label: "Entitlements", icon: "key", command: "russet.openEntitlements" },
        { kind: "editor", label: "Assets", icon: "symbol-color", command: "russet.openAssetCatalog" },
        { kind: "editor", label: "Signing & Capabilities", icon: "verified", command: "russet.openCapabilities" },
      ];
    }
    if (node.kind === "group") {
      return node.items.map((label) => ({
        kind: "editor" as const,
        label,
        icon: "settings-gear",
        command: "russet.selectConfiguration",
      }));
    }
    return [];
  }
}

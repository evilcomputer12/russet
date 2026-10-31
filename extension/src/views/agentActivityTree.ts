// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Agent Activity view (ide.md §5.6). Tier 0 is a placeholder; the MCP tool-call
// feed and approval badge land with EXT-17 / AGT-01. Kept so the manifest id resolves.

import * as vscode from "vscode";

type Node = { label: string; icon: string };

export class AgentActivityTreeProvider implements vscode.TreeDataProvider<Node> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  getTreeItem(node: Node): vscode.TreeItem {
    const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.None);
    item.iconPath = new vscode.ThemeIcon(node.icon);
    return item;
  }

  getChildren(): Node[] {
    return [{ label: "No agent activity yet", icon: "info" }];
  }
}

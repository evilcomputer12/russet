// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Run Configurations view (ide.md §5.6, EXT-04): `launch.json` entries of
// type `russet` plus a synthesized default. Inline Run and Debug actions.

import * as vscode from "vscode";
import type { Store } from "../state/store";

interface ConfigNode {
  name: string;
  synthesized: boolean;
  request: string;
}

export class RunConfigsTreeProvider implements vscode.TreeDataProvider<ConfigNode> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  constructor(private readonly store: Store) {
    store.onDidChange(() => this._onDidChangeTreeData.fire());
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("launch")) {
        this._onDidChangeTreeData.fire();
      }
    });
  }

  getTreeItem(node: ConfigNode): vscode.TreeItem {
    const item = new vscode.TreeItem(node.name, vscode.TreeItemCollapsibleState.None);
    item.iconPath = new vscode.ThemeIcon(node.request === "attach" ? "debug-alt" : "run");
    item.description = node.synthesized ? "default" : node.request;
    item.contextValue = "russet.runConfig";
    return item;
  }

  getChildren(): ConfigNode[] {
    const folder = vscode.workspace.workspaceFolders?.[0];
    const nodes: ConfigNode[] = [];
    if (folder) {
      const launch = vscode.workspace.getConfiguration("launch", folder.uri);
      const configs = launch.get<Array<{ type?: string; name?: string; request?: string }>>(
        "configurations",
        [],
      );
      for (const c of configs) {
        if ((c.type === "russet" || c.type === "dart") && c.name) {
          nodes.push({ name: c.name, synthesized: false, request: c.request ?? "launch" });
        }
      }
    }
    if (nodes.length === 0 && this.store.state.project) {
      const target = this.store.state.selectedTarget ?? this.store.state.project.name;
      nodes.push({ name: `Run ${target} on iPhone`, synthesized: true, request: "launch" });
    }
    return nodes;
  }
}

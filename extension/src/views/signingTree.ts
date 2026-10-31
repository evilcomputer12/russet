// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The Signing view (ide.md §5.6, EXT-04): account/team, certificates and profiles.
// Tier 0 shows the account from `account.list`; the full certificate/profile detail
// (SIGN `signing.status`) is filled when that contract lands.

import * as vscode from "vscode";
import type { RussetClient } from "../daemon/client";
import type { Account } from "../generated/protocol";

type Node =
  | { kind: "account"; account: Account }
  | { kind: "message"; label: string; icon: string };

export class SigningTreeProvider implements vscode.TreeDataProvider<Node> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;
  private accounts: Account[] = [];

  constructor(private readonly client: RussetClient) {
    client.onDidChangeState((s) => {
      if (s === "connected") {
        void this.refresh();
      }
    });
  }

  async refresh(): Promise<void> {
    try {
      const result = await this.client.call("account.list", {});
      this.accounts = result.accounts;
    } catch {
      this.accounts = [];
    }
    this._onDidChangeTreeData.fire();
  }

  getTreeItem(node: Node): vscode.TreeItem {
    if (node.kind === "message") {
      const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.None);
      item.iconPath = new vscode.ThemeIcon(node.icon);
      return item;
    }
    const a = node.account;
    const item = new vscode.TreeItem(a.label, vscode.TreeItemCollapsibleState.None);
    item.description = `${a.kind}${a.teamId ? ` · ${a.teamId}` : ""}${a.isDefault ? " · default" : ""}`;
    item.iconPath = new vscode.ThemeIcon("account");
    item.contextValue = "russet.signingAccount";
    return item;
  }

  getChildren(): Node[] {
    if (this.client.state !== "connected") {
      return [{ kind: "message", label: "Daemon not connected", icon: "circle-slash" }];
    }
    if (this.accounts.length === 0) {
      return [{ kind: "message", label: "No accounts — Add API Key…", icon: "add" }];
    }
    return this.accounts.map((account) => ({ kind: "account" as const, account }));
  }
}

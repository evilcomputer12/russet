// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The native Apple ID sign-in page (SIGN §13.1) as an editor tab. The page posts
// the Apple ID and password once; they go straight into the controller's
// `account.appleId.begin` call and are never logged, stored or echoed back.

import * as vscode from "vscode";
import { localResourceRoots, renderPage } from "../panels/webviewHost";
import type { SignInController } from "./appleId";

export class SignInPanel implements vscode.Disposable {
  public static readonly viewType = "russet.signIn";

  private panel?: vscode.WebviewPanel;
  private readonly disposables: vscode.Disposable[] = [];

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly controller: SignInController,
  ) {
    const sub = controller.onDidChange((view) => void this.panel?.webview.postMessage({ kind: "view", view }));
    this.disposables.push({ dispose: () => sub.dispose() });
  }

  show(): void {
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active);
      return;
    }
    const panel = vscode.window.createWebviewPanel(
      SignInPanel.viewType,
      "Sign in with Apple ID",
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        localResourceRoots: localResourceRoots(this.extensionUri),
        // Keeps the two-factor step alive while the user switches to their phone.
        retainContextWhenHidden: true,
        enableFindWidget: false,
      },
    );
    panel.iconPath = vscode.Uri.joinPath(this.extensionUri, "media", "russet.svg");
    panel.webview.html = renderPage(panel.webview, this.extensionUri, { page: "signin", title: "Sign in with Apple ID" });
    this.panel = panel;
    panel.webview.onDidReceiveMessage((msg: unknown) => void this.onMessage(msg), undefined, this.disposables);
    panel.onDidDispose(() => {
      this.panel = undefined;
    }, undefined, this.disposables);
  }

  private async onMessage(msg: unknown): Promise<void> {
    if (!msg || typeof msg !== "object") {
      return;
    }
    const m = msg as Record<string, unknown>;
    const c = this.controller;
    switch (m.kind) {
      case "ready":
        void this.panel?.webview.postMessage({ kind: "view", view: c.view });
        if (c.view.screen === "checking" || c.view.screen === "unavailable" || c.view.screen === "done") {
          await c.start();
        }
        break;
      case "restart":
        await c.start();
        break;
      case "consent":
        await c.acceptConsent();
        break;
      case "credentials":
        if (typeof m.appleId === "string" && typeof m.password === "string") {
          await c.submitCredentials(m.appleId, m.password);
        }
        break;
      case "code":
        if (typeof m.code === "string") {
          await c.submitCode(m.code);
        }
        break;
      case "resend":
        await c.resend();
        break;
      case "choosePhone":
        c.choosePhone();
        break;
      case "phone":
        if (typeof m.id === "string") {
          await c.textMe(m.id);
        }
        break;
      case "trustedDevice":
        await c.useTrustedDevice();
        break;
      case "back":
        c.back();
        break;
      case "openIdentitySetting":
        await vscode.commands.executeCommand("workbench.action.openSettings", "russet.account.anisetteServer");
        break;
      case "addKey":
        await vscode.commands.executeCommand("russet.account.addAscKey");
        break;
      case "openSigning":
        await vscode.commands.executeCommand("russet.signing.focus");
        break;
      case "openExternal":
        if (m.target === "iforgot") {
          await vscode.env.openExternal(vscode.Uri.parse("https://iforgot.apple.com/"));
        } else if (m.target === "appleid") {
          await vscode.env.openExternal(vscode.Uri.parse("https://account.apple.com/"));
        }
        break;
      case "close":
        this.panel?.dispose();
        break;
      default:
        break;
    }
  }

  dispose(): void {
    this.panel?.dispose();
    for (const d of this.disposables.splice(0)) {
      d.dispose();
    }
  }
}

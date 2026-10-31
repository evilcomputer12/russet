// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The guided setup page as an editor tab (W19). Renders SetupModel snapshots
// and turns the page's messages into model calls. Every message from the page
// is validated: step ids must be in the current status, and an Xcode archive
// path is accepted only if the page found it or the user picked it in Browse.

import * as vscode from "vscode";
import { localResourceRoots, renderPage } from "../panels/webviewHost";
import type { SetupModel } from "./model";
import { DEFAULT_VIRTUAL, pairingsFor, virtualDevices } from "./virtualCatalog";
import { downloadsDir } from "./xcode";

const APPLE_DEVICES_STORE = "https://apps.microsoft.com/detail/9NP83LWLPZ9K";
const XCODE_DOWNLOADS = "https://developer.apple.com/download/all/?q=xcode";
const POST_INTERVAL_MS = 100;

export class SetupPanel implements vscode.Disposable {
  public static readonly viewType = "russet.setup";

  private panel?: vscode.WebviewPanel;
  private readonly disposables: vscode.Disposable[] = [];
  private postTimer?: ReturnType<typeof setTimeout>;
  private lastPost = 0;
  private pendingFocus?: string;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly model: SetupModel,
  ) {
    const sub = model.onDidChange(() => this.schedulePost());
    this.disposables.push({ dispose: () => sub.dispose() });
  }

  get isOpen(): boolean {
    return !!this.panel;
  }

  /** Open (or reveal) the page, optionally scrolled to one step. */
  show(focusStep?: string): void {
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active);
      if (focusStep) {
        void this.panel.webview.postMessage({ kind: "focus", id: focusStep });
      }
      void this.model.refresh();
      return;
    }
    this.pendingFocus = focusStep;
    const panel = vscode.window.createWebviewPanel(
      SetupPanel.viewType,
      "Set up Russet",
      vscode.ViewColumn.Active,
      { enableScripts: true, localResourceRoots: localResourceRoots(this.extensionUri), retainContextWhenHidden: false },
    );
    this.attach(panel);
  }

  /** Adopt a panel (new, or restored by the serializer after a reload). */
  attach(panel: vscode.WebviewPanel): void {
    panel.webview.options = { enableScripts: true, localResourceRoots: localResourceRoots(this.extensionUri) };
    panel.iconPath = vscode.Uri.joinPath(this.extensionUri, "media", "russet.svg");
    panel.webview.html = renderPage(panel.webview, this.extensionUri, { page: "setup", title: "Set up Russet" });
    this.panel = panel;
    panel.webview.onDidReceiveMessage((msg: unknown) => void this.onMessage(msg), undefined, this.disposables);
    panel.onDidChangeViewState((e) => {
      if (e.webviewPanel.visible) {
        void this.model.refresh();
      }
    }, undefined, this.disposables);
    panel.onDidDispose(() => {
      this.panel = undefined;
    }, undefined, this.disposables);
  }

  private schedulePost(): void {
    if (!this.panel || this.postTimer) {
      return;
    }
    const wait = Math.max(0, POST_INTERVAL_MS - (Date.now() - this.lastPost));
    this.postTimer = setTimeout(() => {
      this.postTimer = undefined;
      this.postState();
    }, wait);
  }

  private postState(): void {
    this.lastPost = Date.now();
    void this.panel?.webview.postMessage({ kind: "state", snapshot: this.model.snapshot() });
  }

  private async onMessage(msg: unknown): Promise<void> {
    if (!msg || typeof msg !== "object") {
      return;
    }
    const m = msg as Record<string, unknown>;
    switch (m.kind) {
      case "ready": {
        const devices = virtualDevices();
        void this.panel?.webview.postMessage({
          kind: "catalog",
          catalog: {
            devices: devices.map((d) => ({ id: d.id, name: d.name })),
            pairings: devices.flatMap((d) => pairingsFor(d.id)),
            defaultChoice: DEFAULT_VIRTUAL,
          },
        });
        this.postState();
        if (this.pendingFocus) {
          void this.panel?.webview.postMessage({ kind: "focus", id: this.pendingFocus });
          this.pendingFocus = undefined;
        }
        void this.model.refresh();
        break;
      }
      case "refresh":
        await this.model.refresh();
        break;
      case "run": {
        const mode = m.mode === "retry" || m.mode === "repair" ? m.mode : "run";
        if (typeof m.id === "string") {
          const outcome = await this.model.runStep(m.id, mode);
          if (outcome?.kind === "failed") {
            void this.panel?.webview.postMessage({ kind: "announce", text: `Step failed: ${outcome.message}` });
          }
        }
        break;
      }
      case "runAll":
        await this.model.runAll();
        break;
      case "cancel":
        this.model.cancel();
        break;
      case "selectXip":
        if (typeof m.path === "string") {
          await this.model.selectXip(m.path);
        }
        break;
      case "browseXip":
        await this.browseXip();
        break;
      case "selectVirtual":
        if (typeof m.device === "string" && typeof m.ios === "string") {
          await this.model.setVirtualChoice({ device: m.device, ios: m.ios });
        }
        break;
      case "signIn":
        await vscode.commands.executeCommand("russet.account.signInAppleId");
        break;
      case "pickDevice":
        await vscode.commands.executeCommand("russet.selectDevice");
        await this.model.refresh();
        break;
      case "addKey":
        await vscode.commands.executeCommand("russet.account.addAscKey");
        await this.model.refresh();
        break;
      case "copyWslCommand":
        await vscode.env.clipboard.writeText("wsl --install --no-distribution");
        void this.panel?.webview.postMessage({ kind: "announce", text: "Command copied." });
        break;
      case "openExternal":
        if (m.target === "apple-devices") {
          await vscode.env.openExternal(vscode.Uri.parse(APPLE_DEVICES_STORE));
        } else if (m.target === "xcode-download") {
          await vscode.env.openExternal(vscode.Uri.parse(XCODE_DOWNLOADS));
        }
        break;
      case "openWalkthrough":
        await vscode.commands.executeCommand("russet.setup.openWalkthrough");
        break;
      default:
        break;
    }
  }

  private async browseXip(): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      title: "Choose the Xcode archive you downloaded from Apple",
      defaultUri: vscode.Uri.file(downloadsDir()),
      canSelectMany: false,
      canSelectFolders: false,
      filters: { "Xcode archive": ["xip"] },
      openLabel: "Use This Archive",
    });
    if (picked && picked.length > 0) {
      const ok = await this.model.browseXip(picked[0].fsPath);
      if (!ok) {
        void vscode.window.showErrorMessage("Choose an Xcode .xip archive.");
      }
    }
  }

  dispose(): void {
    if (this.postTimer) {
      clearTimeout(this.postTimer);
    }
    this.panel?.dispose();
    for (const d of this.disposables.splice(0)) {
      d.dispose();
    }
  }
}

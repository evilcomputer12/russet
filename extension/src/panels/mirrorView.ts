// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The embedded "iPhone" view (ide.md §5.10). Tier 0 is a placeholder panel where
// the live screen mirror / virtual-iPhone emulator will render; the decode pipeline
// and HID input land with WS-12 (mirror.start → ws://127.0.0.1). The panel keeps a
// backend-neutral contract so a "virtual device" source can slot in unchanged.

import * as vscode from "vscode";
import type { Store } from "../state/store";
import { renderPage, localResourceRoots } from "./webviewHost";

export interface MirrorTarget {
  udid: string;
  name: string;
  /** "device" today; "virtual" when the embedded emulator drives the panel. */
  kind: "device" | "virtual";
  display?: { width: number; height: number; scale: number };
}

export class MirrorViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewId = "russet.mirror";

  private view?: vscode.WebviewView;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly store: Store,
  ) {
    store.onDidChangeActiveDevice(() => this.pushState());
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: localResourceRoots(this.extensionUri),
    };
    view.webview.html = renderPage(view.webview, this.extensionUri, {
      page: "mirror",
      title: "iPhone",
    });
    view.webview.onDidReceiveMessage((msg: unknown) => {
      if (msg && typeof msg === "object" && (msg as { kind?: string }).kind === "ready") {
        this.pushState();
      }
    });
    view.onDidDispose(() => {
      this.view = undefined;
    });
  }

  async reveal(): Promise<void> {
    await vscode.commands.executeCommand("russet.mirror.focus");
    this.pushState();
  }

  private target(): MirrorTarget | undefined {
    const d = this.store.activeDevice;
    if (!d) {
      return undefined;
    }
    return { udid: d.udid, name: d.marketingName ?? d.name, kind: "device" };
  }

  private pushState(): void {
    const target = this.target();
    void this.view?.webview.postMessage({
      kind: "state",
      // Tier 0: "placeholder" until the mirror stream (WS-12) is wired.
      status: "placeholder",
      target: target ? { name: target.name, kind: target.kind } : undefined,
    });
  }
}

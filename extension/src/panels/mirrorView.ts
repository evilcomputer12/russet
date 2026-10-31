// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// The embedded "iPhone" sidebar view (ide.md §5.10): a compact device frame for
// the run destination, physical or virtual. The interactive screen is the
// "Virtual iPhone" editor tab (virtualIphonePanel.ts), which shares the frame;
// the decode pipeline and HID input land with EXT-09 (mirror.start →
// ws://127.0.0.1). The message contract is backend-neutral (device vs virtual).

import * as vscode from "vscode";
import type { Store } from "../state/store";
import type { DestinationModel } from "./destinations";
import { renderPage, localResourceRoots } from "./webviewHost";

export interface MirrorTarget {
  udid: string;
  name: string;
  /** "device" for a USB/Wi-Fi iPhone; "virtual" for the embedded virtual iPhone. */
  kind: "device" | "virtual";
  display?: { width: number; height: number; scale: number };
}

export class MirrorViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewId = "russet.mirror";

  private view?: vscode.WebviewView;
  private destinations?: DestinationModel;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly store: Store,
  ) {
    store.onDidChangeActiveDevice(() => this.pushState());
  }

  /** Follow the Xcode-style destination picker (physical and virtual). */
  useDestinations(destinations: DestinationModel): void {
    this.destinations = destinations;
    destinations.onDidChange(() => this.pushState());
    this.pushState();
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
      const kind = msg && typeof msg === "object" ? (msg as { kind?: string }).kind : undefined;
      if (kind === "ready") {
        this.pushState();
      } else if (kind === "openEditor") {
        void vscode.commands.executeCommand("russet.openVirtualIphone");
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

  private target(): (MirrorTarget & { detail: string; screen?: [number, number] }) | undefined {
    const dest = this.destinations?.selected;
    if (dest) {
      return {
        udid: dest.id,
        name: dest.name,
        kind: dest.kind === "virtual" ? "virtual" : "device",
        detail: [dest.osVersion ? `iOS ${dest.osVersion}` : "", dest.stateLabel].filter(Boolean).join(" · "),
        ...(dest.screen ? { screen: dest.screen } : {}),
      };
    }
    const d = this.store.activeDevice;
    if (!d) {
      return undefined;
    }
    return { udid: d.udid, name: d.marketingName ?? d.name, kind: "device", detail: `iOS ${d.osVersion}` };
  }

  private pushState(): void {
    const target = this.target();
    void this.view?.webview.postMessage({
      kind: "state",
      // "placeholder" until the mirror stream (EXT-09) is wired.
      status: "placeholder",
      target: target
        ? { name: target.name, kind: target.kind, detail: target.detail, ...(target.screen ? { screen: target.screen } : {}) }
        : undefined,
    });
  }
}
